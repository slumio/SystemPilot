#!/bin/sh
set -eu

fail() { printf 'SysPilot fleet DB setup: %s\n' "$*" >&2; exit 1; }

: "${FLEET_DATABASE_URL:?Set FLEET_DATABASE_URL to an administrative PostgreSQL connection URI}"
command -v psql >/dev/null 2>&1 || fail "psql is required"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"

case "$FLEET_DATABASE_URL" in
  *sslmode=verify-full*|*sslmode=verify-ca*|*sslmode=require*) ;;
  *) [ "${FLEET_DB_ALLOW_INSECURE:-0}" = "1" ] || fail "database URI must require TLS; set FLEET_DB_ALLOW_INSECURE=1 only for an isolated local development database" ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
migration="$script_dir/postgres/001_initial.sql"
[ -r "$migration" ] || fail "migration is missing: $migration"
checksum=$(sha256sum "$migration" | awk '{print $1}')

schema_exists=$(psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --tuples-only --no-align \
  --command "SELECT to_regclass('syspilot_control.schema_migrations') IS NOT NULL") || fail "could not inspect migration schema"
schema_exists=$(printf '%s' "$schema_exists" | tr -d '[:space:]')
existing=""
if [ "$schema_exists" = "t" ]; then
  existing=$(psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --tuples-only --no-align \
    --command "SELECT COALESCE((SELECT checksum_sha256 FROM syspilot_control.schema_migrations WHERE version = 1), '')") || fail "could not inspect migration ledger"
  existing=$(printf '%s' "$existing" | tr -d '[:space:]')
fi
if [ -n "$existing" ]; then
  [ "$existing" = "$checksum" ] || fail "migration version 1 checksum mismatch; refusing to modify the database"
  printf 'Fleet database schema version 1 is already current.\n'
  exit 0
fi

psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --file "$migration" || fail "schema migration failed and was rolled back"
psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --set migration_checksum="$checksum" \
  --command "INSERT INTO syspilot_control.schema_migrations(version, checksum_sha256) VALUES (1, :'migration_checksum')" \
  >/dev/null || fail "schema was applied but migration ledger update failed; operator reconciliation is required"
printf 'Fleet database schema version 1 applied successfully.\n'
printf 'Next: create a LOGIN role and grant it membership in syspilot_control_app; never use the migration administrator at runtime.\n'
