#!/bin/sh
set -eu

: "${FLEET_DATABASE_URL:?}"
: "${SYSPILOT_DEV_PEPPER:?}"
: "${SYSPILOT_DEV_TOKEN:?}"
: "${SYSPILOT_DEV_RUNTIME_PASSWORD:?}"

case "$SYSPILOT_DEV_RUNTIME_PASSWORD" in
    ''|*[!A-Za-z0-9]*) printf 'development runtime password must be alphanumeric\n' >&2; exit 2 ;;
esac
[ "${#SYSPILOT_DEV_RUNTIME_PASSWORD}" -ge 24 ] || { printf 'development runtime password must contain at least 24 characters\n' >&2; exit 2; }

/deploy/fleet/setup-db.sh

psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 \
    --set pepper="$SYSPILOT_DEV_PEPPER" --set token="$SYSPILOT_DEV_TOKEN" \
    --set runtime_password="$SYSPILOT_DEV_RUNTIME_PASSWORD" <<'SQL'
DO $$ BEGIN
    CREATE ROLE syspilot_runtime LOGIN;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;
ALTER ROLE syspilot_runtime PASSWORD :'runtime_password';
GRANT syspilot_control_app TO syspilot_runtime;

INSERT INTO syspilot_control.tenants(tenant_id,name)
VALUES ('00000000-0000-0000-0000-000000000001','Local development')
ON CONFLICT (tenant_id) DO NOTHING;
INSERT INTO syspilot_control.nodes(tenant_id,node_id,agent_version,protocol_version)
VALUES ('00000000-0000-0000-0000-000000000001','local-node','development',1)
ON CONFLICT (tenant_id,node_id) DO NOTHING;
INSERT INTO syspilot_control.enrollment_credentials
    (tenant_id,node_id,token_prefix,token_hash,expires_at)
SELECT '00000000-0000-0000-0000-000000000001','local-node','spn_local',
       hmac(:'token', :'pepper', 'sha256'), clock_timestamp() + interval '30 days'
WHERE NOT EXISTS (
    SELECT 1 FROM syspilot_control.enrollment_credentials
    WHERE tenant_id='00000000-0000-0000-0000-000000000001' AND node_id='local-node'
);
SQL
