BEGIN;

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE SCHEMA IF NOT EXISTS syspilot_control;
REVOKE ALL ON SCHEMA syspilot_control FROM PUBLIC;

DO $$ BEGIN
    CREATE ROLE syspilot_control_app NOLOGIN;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

CREATE TABLE IF NOT EXISTS syspilot_control.schema_migrations (
    version bigint PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    checksum_sha256 text NOT NULL CHECK (checksum_sha256 ~ '^[0-9a-f]{64}$')
);

CREATE TABLE IF NOT EXISTS syspilot_control.tenants (
    tenant_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspended','deleting')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE IF NOT EXISTS syspilot_control.principals (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    principal_id uuid NOT NULL DEFAULT gen_random_uuid(),
    external_subject text NOT NULL CHECK (length(btrim(external_subject)) BETWEEN 1 AND 500),
    role text NOT NULL CHECK (role IN ('admin','operator','viewer')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, principal_id),
    UNIQUE (tenant_id, external_subject)
);

CREATE TABLE IF NOT EXISTS syspilot_control.nodes (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    node_id text NOT NULL CHECK (length(btrim(node_id)) BETWEEN 1 AND 255),
    display_name text,
    environment text,
    region text,
    agent_version text NOT NULL,
    protocol_version integer NOT NULL CHECK (protocol_version > 0),
    capabilities jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(capabilities) = 'object'),
    labels jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(labels) = 'object'),
    enrolled_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    last_seen_at timestamptz,
    deleted_at timestamptz,
    PRIMARY KEY (tenant_id, node_id)
);

CREATE TABLE IF NOT EXISTS syspilot_control.enrollment_credentials (
    tenant_id uuid NOT NULL,
    credential_id uuid NOT NULL DEFAULT gen_random_uuid(),
    node_id text NOT NULL,
    token_prefix text NOT NULL CHECK (length(token_prefix) BETWEEN 6 AND 32),
    token_hash bytea NOT NULL CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    rotated_from uuid,
    PRIMARY KEY (tenant_id, credential_id),
    UNIQUE (tenant_id, token_hash),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE CASCADE,
    CHECK (expires_at > created_at)
);

CREATE TABLE IF NOT EXISTS syspilot_control.telemetry_messages (
    tenant_id uuid NOT NULL,
    node_id text NOT NULL,
    message_id text NOT NULL CHECK (length(btrim(message_id)) BETWEEN 1 AND 500),
    sequence bigint NOT NULL CHECK (sequence > 0),
    schema_version integer NOT NULL CHECK (schema_version > 0),
    kind text NOT NULL,
    observed_at timestamptz NOT NULL,
    received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    envelope jsonb NOT NULL CHECK (jsonb_typeof(envelope) = 'object'),
    PRIMARY KEY (tenant_id, node_id, message_id),
    UNIQUE (tenant_id, node_id, sequence),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS telemetry_received_idx ON syspilot_control.telemetry_messages (tenant_id, received_at DESC);
CREATE INDEX IF NOT EXISTS telemetry_kind_idx ON syspilot_control.telemetry_messages (tenant_id, kind, observed_at DESC);

CREATE TABLE IF NOT EXISTS syspilot_control.node_sequence_state (
    tenant_id uuid NOT NULL,
    node_id text NOT NULL,
    highest_accepted_sequence bigint NOT NULL DEFAULT 0 CHECK (highest_accepted_sequence >= 0),
    gap_ranges int8multirange NOT NULL DEFAULT '{}'::int8multirange,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, node_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS syspilot_control.cases (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    case_id uuid NOT NULL DEFAULT gen_random_uuid(),
    title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 500),
    status text NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved','archived')),
    created_by uuid NOT NULL,
    evidence_object_key text,
    summary jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(summary) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, case_id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES syspilot_control.principals(tenant_id, principal_id)
);

CREATE TABLE IF NOT EXISTS syspilot_control.case_annotations (
    tenant_id uuid NOT NULL,
    case_id uuid NOT NULL,
    annotation_id uuid NOT NULL DEFAULT gen_random_uuid(),
    author_id uuid NOT NULL,
    body text NOT NULL CHECK (length(btrim(body)) BETWEEN 1 AND 20000),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, case_id, annotation_id),
    FOREIGN KEY (tenant_id, case_id) REFERENCES syspilot_control.cases(tenant_id, case_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, author_id) REFERENCES syspilot_control.principals(tenant_id, principal_id)
);

CREATE TABLE IF NOT EXISTS syspilot_control.alerts (
    tenant_id uuid NOT NULL,
    alert_instance_id text NOT NULL,
    node_id text NOT NULL,
    rule_id text NOT NULL,
    state text NOT NULL CHECK (state IN ('firing','acknowledged','resolved','suppressed')),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    first_observed_at timestamptz NOT NULL,
    last_transition_at timestamptz NOT NULL,
    acknowledged_by uuid,
    PRIMARY KEY (tenant_id, alert_instance_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, acknowledged_by) REFERENCES syspilot_control.principals(tenant_id, principal_id)
);
CREATE INDEX IF NOT EXISTS alerts_state_idx ON syspilot_control.alerts (tenant_id, state, last_transition_at DESC);

CREATE TABLE IF NOT EXISTS syspilot_control.retention_policies (
    tenant_id uuid PRIMARY KEY REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    telemetry_days integer NOT NULL DEFAULT 30 CHECK (telemetry_days BETWEEN 1 AND 3650),
    case_days integer NOT NULL DEFAULT 365 CHECK (case_days BETWEEN 1 AND 3650),
    audit_days integer NOT NULL DEFAULT 365 CHECK (audit_days BETWEEN 30 AND 3650),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE IF NOT EXISTS syspilot_control.deletion_requests (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    request_id uuid NOT NULL DEFAULT gen_random_uuid(),
    requested_by uuid NOT NULL,
    scope text NOT NULL CHECK (scope IN ('node','tenant','telemetry','case')),
    target_id text,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','completed','failed')),
    requested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    completed_at timestamptz,
    failure_reason text,
    PRIMARY KEY (tenant_id, request_id),
    FOREIGN KEY (tenant_id, requested_by) REFERENCES syspilot_control.principals(tenant_id, principal_id)
);

CREATE TABLE IF NOT EXISTS syspilot_control.audit_events (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE RESTRICT,
    audit_id bigint GENERATED ALWAYS AS IDENTITY,
    actor_id uuid,
    action text NOT NULL CHECK (length(btrim(action)) BETWEEN 1 AND 200),
    target_type text NOT NULL,
    target_id text,
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    source_ip inet,
    detail jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object'),
    PRIMARY KEY (tenant_id, audit_id),
    FOREIGN KEY (tenant_id, actor_id) REFERENCES syspilot_control.principals(tenant_id, principal_id)
);

CREATE OR REPLACE FUNCTION syspilot_control.current_tenant_id() RETURNS uuid
LANGUAGE sql STABLE PARALLEL SAFE
AS $$ SELECT NULLIF(current_setting('syspilot.tenant_id', true), '')::uuid $$;

CREATE OR REPLACE FUNCTION syspilot_control.reject_audit_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'audit events are immutable'; END $$;
DROP TRIGGER IF EXISTS audit_events_immutable ON syspilot_control.audit_events;
CREATE TRIGGER audit_events_immutable BEFORE UPDATE OR DELETE ON syspilot_control.audit_events
FOR EACH ROW EXECUTE FUNCTION syspilot_control.reject_audit_mutation();

DO $$ DECLARE table_name text; BEGIN
    FOREACH table_name IN ARRAY ARRAY['principals','nodes','enrollment_credentials','telemetry_messages','node_sequence_state','cases','case_annotations','alerts','retention_policies','deletion_requests','audit_events'] LOOP
        EXECUTE format('ALTER TABLE syspilot_control.%I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE syspilot_control.%I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format('DROP POLICY IF EXISTS tenant_isolation ON syspilot_control.%I', table_name);
        EXECUTE format('CREATE POLICY tenant_isolation ON syspilot_control.%I USING (tenant_id = syspilot_control.current_tenant_id()) WITH CHECK (tenant_id = syspilot_control.current_tenant_id())', table_name);
    END LOOP;
END $$;

REVOKE ALL ON ALL TABLES IN SCHEMA syspilot_control FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA syspilot_control FROM PUBLIC;
GRANT USAGE ON SCHEMA syspilot_control TO syspilot_control_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA syspilot_control TO syspilot_control_app;
REVOKE ALL ON syspilot_control.tenants, syspilot_control.schema_migrations FROM syspilot_control_app;
REVOKE UPDATE, DELETE ON syspilot_control.audit_events FROM syspilot_control_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA syspilot_control TO syspilot_control_app;


COMMIT;
