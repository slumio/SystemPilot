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
set -- "$script_dir"/postgres/[0-9][0-9][0-9]_*.sql
[ -r "$1" ] || fail "no migrations found in $script_dir/postgres"

schema_exists=$(psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --tuples-only --no-align \
  --command "SELECT to_regclass('syspilot_control.schema_migrations') IS NOT NULL") || fail "could not inspect migration schema"
schema_exists=$(printf '%s' "$schema_exists" | tr -d '[:space:]')
for migration do
  filename=${migration##*/}
  version_text=${filename%%_*}
  version=$(printf '%s' "$version_text" | sed 's/^0*//')
  [ -n "$version" ] || version=0
  checksum=$(sha256sum "$migration" | awk '{print $1}')
  case "$checksum" in
    ''|*[!0-9a-f]*) fail "migration $filename checksum is not lowercase hexadecimal SHA-256" ;;
  esac
  [ "${#checksum}" -eq 64 ] || fail "migration $filename checksum has an invalid length"

  existing=""
  if [ "$schema_exists" = "t" ]; then
    existing=$(psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --tuples-only --no-align \
      --command "SELECT COALESCE((SELECT checksum_sha256 FROM syspilot_control.schema_migrations WHERE version = $version), '')") || fail "could not inspect migration ledger"
    existing=$(printf '%s' "$existing" | tr -d '[:space:]')
  fi
  if [ -n "$existing" ]; then
    [ "$existing" = "$checksum" ] || fail "migration version $version checksum mismatch; refusing to modify the database"
    printf 'Fleet database migration %s is already applied.\n' "$version"
    continue
  fi

  psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 --file "$migration" || fail "migration $version failed and was rolled back"
  psql "$FLEET_DATABASE_URL" --no-psqlrc --set ON_ERROR_STOP=1 \
    --command "INSERT INTO syspilot_control.schema_migrations(version, checksum_sha256) VALUES ($version, '$checksum')" \
    >/dev/null || fail "migration $version applied but its ledger update failed; operator reconciliation is required"
  schema_exists=t
  printf 'Fleet database migration %s applied successfully.\n' "$version"
done

printf 'Next: create a LOGIN role and grant it membership in syspilot_control_app; never use the migration administrator at runtime.\n'
