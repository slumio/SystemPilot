BEGIN;

ALTER TABLE syspilot_control.telemetry_messages
    ADD COLUMN IF NOT EXISTS envelope_digest bytea;

ALTER TABLE syspilot_control.telemetry_messages
    DROP CONSTRAINT IF EXISTS telemetry_messages_envelope_digest_length;
ALTER TABLE syspilot_control.telemetry_messages
    ADD CONSTRAINT telemetry_messages_envelope_digest_length
    CHECK (envelope_digest IS NULL OR octet_length(envelope_digest) = 32);

COMMIT;
