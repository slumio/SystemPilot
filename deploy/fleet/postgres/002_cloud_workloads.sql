BEGIN;

ALTER TABLE syspilot_control.principals
    DROP CONSTRAINT IF EXISTS principals_role_check;
ALTER TABLE syspilot_control.principals
    ADD CONSTRAINT principals_role_check
    CHECK (role IN ('owner','admin','operator','viewer','billing'));

CREATE TABLE syspilot_control.node_heartbeats (
    tenant_id uuid NOT NULL,
    node_id text NOT NULL,
    observed_at timestamptz NOT NULL,
    agent_version text NOT NULL CHECK (length(agent_version) BETWEEN 1 AND 100),
    protocol_version integer NOT NULL CHECK (protocol_version > 0),
    health jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(health) = 'object'),
    received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, node_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE CASCADE
);

CREATE TABLE syspilot_control.active_server_days (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE RESTRICT,
    usage_day date NOT NULL,
    node_id text NOT NULL,
    first_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    last_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, usage_day, node_id),
    FOREIGN KEY (tenant_id, node_id) REFERENCES syspilot_control.nodes(tenant_id, node_id) ON DELETE RESTRICT
);

CREATE TABLE syspilot_control.reasoning_jobs (
    tenant_id uuid NOT NULL,
    job_id bigint GENERATED ALWAYS AS IDENTITY,
    node_id text NOT NULL,
    message_id text NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','completed','failed','cancelled')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 100),
    available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    leased_until timestamptz,
    failure_code text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    completed_at timestamptz,
    PRIMARY KEY (tenant_id, job_id),
    UNIQUE (tenant_id, node_id, message_id),
    FOREIGN KEY (tenant_id, node_id, message_id)
        REFERENCES syspilot_control.telemetry_messages(tenant_id, node_id, message_id) ON DELETE CASCADE
);
CREATE INDEX reasoning_jobs_pending_idx ON syspilot_control.reasoning_jobs
    (available_at, tenant_id) WHERE status = 'pending';

CREATE TABLE syspilot_control.reasoning_results (
    tenant_id uuid NOT NULL,
    job_id bigint NOT NULL,
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, job_id),
    FOREIGN KEY (tenant_id, job_id) REFERENCES syspilot_control.reasoning_jobs(tenant_id, job_id) ON DELETE CASCADE
);

CREATE TABLE syspilot_control.notification_deliveries (
    tenant_id uuid NOT NULL,
    delivery_id bigint GENERATED ALWAYS AS IDENTITY,
    alert_instance_id text NOT NULL,
    channel text NOT NULL CHECK (channel IN ('email','webhook')),
    destination_ref text NOT NULL CHECK (length(destination_ref) BETWEEN 1 AND 500),
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','sending','delivered','failed','cancelled')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND 100),
    available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    delivered_at timestamptz,
    failure_code text,
    PRIMARY KEY (tenant_id, delivery_id),
    FOREIGN KEY (tenant_id, alert_instance_id)
        REFERENCES syspilot_control.alerts(tenant_id, alert_instance_id) ON DELETE CASCADE
);
CREATE INDEX notification_pending_idx ON syspilot_control.notification_deliveries
    (available_at, tenant_id) WHERE status = 'pending';

CREATE TABLE syspilot_control.alert_destinations (
    tenant_id uuid NOT NULL REFERENCES syspilot_control.tenants(tenant_id) ON DELETE CASCADE,
    destination_id uuid NOT NULL DEFAULT gen_random_uuid(),
    channel text NOT NULL CHECK (channel IN ('email','webhook')),
    destination_ref text NOT NULL CHECK (length(destination_ref) BETWEEN 1 AND 500),
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, destination_id),
    UNIQUE (tenant_id, channel, destination_ref)
);

CREATE OR REPLACE FUNCTION syspilot_control.authenticate_node(p_token_hash bytea)
RETURNS TABLE(tenant_id uuid, node_id text)
LANGUAGE sql
SECURITY DEFINER
STABLE
SET search_path = pg_catalog, syspilot_control
AS $$
    SELECT credential.tenant_id, credential.node_id
    FROM syspilot_control.enrollment_credentials AS credential
    JOIN syspilot_control.tenants AS tenant USING (tenant_id)
    JOIN syspilot_control.nodes AS node
      ON node.tenant_id = credential.tenant_id AND node.node_id = credential.node_id
    WHERE credential.token_hash = p_token_hash
      AND credential.revoked_at IS NULL
      AND credential.expires_at > clock_timestamp()
      AND tenant.status = 'active'
      AND node.deleted_at IS NULL
$$;

REVOKE ALL ON FUNCTION syspilot_control.authenticate_node(bytea) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION syspilot_control.authenticate_node(bytea) TO syspilot_control_app;

DO $$ BEGIN
    CREATE ROLE syspilot_cloud_worker NOLOGIN;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

CREATE OR REPLACE FUNCTION syspilot_control.lease_reasoning_job()
RETURNS TABLE(tenant_id uuid, job_id bigint, node_id text, message_id text, envelope jsonb)
LANGUAGE sql
SECURITY DEFINER
VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    WITH candidate AS (
        SELECT job.tenant_id, job.job_id
        FROM syspilot_control.reasoning_jobs AS job
        WHERE (job.status = 'pending' AND job.available_at <= clock_timestamp())
           OR (job.status = 'running' AND job.leased_until < clock_timestamp())
        ORDER BY job.available_at, job.job_id
        FOR UPDATE SKIP LOCKED
        LIMIT 1
    ), leased AS (
        UPDATE syspilot_control.reasoning_jobs AS job
        SET status='running', leased_until=clock_timestamp() + interval '30 seconds',
            attempt_count=job.attempt_count + 1
        FROM candidate
        WHERE job.tenant_id=candidate.tenant_id AND job.job_id=candidate.job_id
        RETURNING job.tenant_id,job.job_id,job.node_id,job.message_id
    )
    SELECT leased.tenant_id,leased.job_id,leased.node_id,leased.message_id,message.envelope
    FROM leased
    JOIN syspilot_control.telemetry_messages AS message
      ON message.tenant_id=leased.tenant_id AND message.node_id=leased.node_id
     AND message.message_id=leased.message_id
$$;

CREATE OR REPLACE FUNCTION syspilot_control.complete_reasoning_job(
    p_tenant_id uuid, p_job_id bigint, p_result jsonb
) RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$ BEGIN
    IF jsonb_typeof(p_result) <> 'object' THEN
        RAISE EXCEPTION 'reasoning result must be an object';
    END IF;
    INSERT INTO syspilot_control.reasoning_results(tenant_id,job_id,result)
    VALUES (p_tenant_id,p_job_id,p_result)
    ON CONFLICT (tenant_id,job_id) DO UPDATE SET result=EXCLUDED.result;
    UPDATE syspilot_control.reasoning_jobs
    SET status='completed',completed_at=clock_timestamp(),leased_until=NULL,failure_code=NULL
    WHERE tenant_id=p_tenant_id AND job_id=p_job_id AND status='running';
    RETURN FOUND;
END $$;

CREATE OR REPLACE FUNCTION syspilot_control.fail_reasoning_job(
    p_tenant_id uuid, p_job_id bigint, p_failure_code text
) RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    UPDATE syspilot_control.reasoning_jobs
    SET status=CASE WHEN attempt_count >= 10 THEN 'failed' ELSE 'pending' END,
        available_at=clock_timestamp() + make_interval(secs => LEAST(300, power(2, LEAST(attempt_count,8))::integer)),
        leased_until=NULL,
        failure_code=left(p_failure_code,100)
    WHERE tenant_id=p_tenant_id AND job_id=p_job_id AND status='running'
    RETURNING true
$$;

REVOKE ALL ON FUNCTION syspilot_control.lease_reasoning_job() FROM PUBLIC;
REVOKE ALL ON FUNCTION syspilot_control.complete_reasoning_job(uuid,bigint,jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION syspilot_control.fail_reasoning_job(uuid,bigint,text) FROM PUBLIC;
GRANT USAGE ON SCHEMA syspilot_control TO syspilot_cloud_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.lease_reasoning_job() TO syspilot_cloud_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.complete_reasoning_job(uuid,bigint,jsonb) TO syspilot_cloud_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.fail_reasoning_job(uuid,bigint,text) TO syspilot_cloud_worker;

DO $$ DECLARE table_name text; BEGIN
    FOREACH table_name IN ARRAY ARRAY['node_heartbeats','active_server_days','reasoning_jobs','reasoning_results','notification_deliveries','alert_destinations'] LOOP
        EXECUTE format('ALTER TABLE syspilot_control.%I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE syspilot_control.%I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format('CREATE POLICY tenant_isolation ON syspilot_control.%I USING (tenant_id = syspilot_control.current_tenant_id()) WITH CHECK (tenant_id = syspilot_control.current_tenant_id())', table_name);
    END LOOP;
END $$;

GRANT SELECT, INSERT, UPDATE, DELETE ON
    syspilot_control.node_heartbeats,
    syspilot_control.active_server_days,
    syspilot_control.reasoning_jobs,
    syspilot_control.reasoning_results,
    syspilot_control.notification_deliveries,
    syspilot_control.alert_destinations
TO syspilot_control_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA syspilot_control TO syspilot_control_app;

COMMIT;
