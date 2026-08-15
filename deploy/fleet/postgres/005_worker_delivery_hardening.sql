BEGIN;

ALTER TABLE syspilot_control.notification_deliveries
    ADD COLUMN IF NOT EXISTS created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    ADD COLUMN IF NOT EXISTS leased_until timestamptz;

CREATE OR REPLACE FUNCTION syspilot_control.lease_notification_delivery()
RETURNS TABLE(tenant_id uuid, delivery_id bigint, alert_instance_id text, channel text, destination_ref text, payload jsonb)
LANGUAGE sql SECURITY DEFINER VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    WITH candidate AS (
        SELECT delivery.tenant_id,delivery.delivery_id
        FROM syspilot_control.notification_deliveries AS delivery
        WHERE (delivery.status='pending' AND delivery.available_at <= clock_timestamp())
           OR (delivery.status='sending' AND delivery.leased_until < clock_timestamp())
        ORDER BY delivery.available_at,delivery.delivery_id
        FOR UPDATE SKIP LOCKED LIMIT 1
    ), leased AS (
        UPDATE syspilot_control.notification_deliveries AS delivery
        SET status='sending',attempt_count=delivery.attempt_count+1,
            leased_until=clock_timestamp() + interval '30 seconds'
        FROM candidate
        WHERE delivery.tenant_id=candidate.tenant_id AND delivery.delivery_id=candidate.delivery_id
        RETURNING delivery.tenant_id,delivery.delivery_id,delivery.alert_instance_id,
                  delivery.channel,delivery.destination_ref
    )
    SELECT leased.tenant_id,leased.delivery_id,leased.alert_instance_id,
           leased.channel,leased.destination_ref,alert.payload
    FROM leased JOIN syspilot_control.alerts AS alert
      ON alert.tenant_id=leased.tenant_id AND alert.alert_instance_id=leased.alert_instance_id
$$;

CREATE OR REPLACE FUNCTION syspilot_control.complete_notification_delivery(
    p_tenant_id uuid,p_delivery_id bigint
)
RETURNS boolean LANGUAGE sql SECURITY DEFINER VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    UPDATE syspilot_control.notification_deliveries
    SET status='delivered',delivered_at=clock_timestamp(),failure_code=NULL,leased_until=NULL
    WHERE tenant_id=p_tenant_id AND delivery_id=p_delivery_id AND status='sending' RETURNING true
$$;

CREATE OR REPLACE FUNCTION syspilot_control.fail_notification_delivery(
    p_tenant_id uuid,p_delivery_id bigint,p_failure_code text
)
RETURNS boolean LANGUAGE sql SECURITY DEFINER VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    UPDATE syspilot_control.notification_deliveries
    SET status=CASE WHEN attempt_count >= 10 THEN 'failed' ELSE 'pending' END,
        available_at=clock_timestamp() + make_interval(secs => LEAST(300,power(2,LEAST(attempt_count,8))::integer)),
        failure_code=left(p_failure_code,100),leased_until=NULL
    WHERE tenant_id=p_tenant_id AND delivery_id=p_delivery_id AND status='sending' RETURNING true
$$;

CREATE OR REPLACE VIEW syspilot_control.worker_delivery_health AS
SELECT 'reasoning'::text AS queue,
       count(*) FILTER (WHERE status IN ('pending','running')) AS queued,
       COALESCE(max(extract(epoch FROM (clock_timestamp()-created_at))) FILTER (WHERE status IN ('pending','running')),0)::bigint AS oldest_queue_age_seconds,
       count(*) FILTER (WHERE status='running' AND leased_until < clock_timestamp()) AS expired_leases,
       count(*) FILTER (WHERE status='failed') AS failed
FROM syspilot_control.reasoning_jobs
UNION ALL
SELECT 'notification',count(*) FILTER (WHERE status IN ('pending','sending')),
       COALESCE(max(extract(epoch FROM (clock_timestamp()-created_at))) FILTER (WHERE status IN ('pending','sending')),0)::bigint,
       count(*) FILTER (WHERE status='sending' AND leased_until < clock_timestamp()),
       count(*) FILTER (WHERE status='failed')
FROM syspilot_control.notification_deliveries;

REVOKE ALL ON syspilot_control.worker_delivery_health FROM PUBLIC;
GRANT SELECT ON syspilot_control.worker_delivery_health TO syspilot_cloud_worker,syspilot_notification_worker;

COMMIT;
