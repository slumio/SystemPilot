BEGIN;

DO $$ BEGIN
    CREATE ROLE syspilot_notification_worker NOLOGIN;
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

CREATE FUNCTION syspilot_control.lease_notification_delivery()
RETURNS TABLE(tenant_id uuid, delivery_id bigint, alert_instance_id text, channel text, destination_ref text, payload jsonb)
LANGUAGE sql
SECURITY DEFINER
VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    WITH candidate AS (
        SELECT delivery.tenant_id,delivery.delivery_id
        FROM syspilot_control.notification_deliveries AS delivery
        WHERE delivery.status='pending' AND delivery.available_at <= clock_timestamp()
        ORDER BY delivery.available_at,delivery.delivery_id
        FOR UPDATE SKIP LOCKED LIMIT 1
    ), leased AS (
        UPDATE syspilot_control.notification_deliveries AS delivery
        SET status='sending',attempt_count=delivery.attempt_count+1
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

CREATE FUNCTION syspilot_control.complete_notification_delivery(
    p_tenant_id uuid,p_delivery_id bigint
) RETURNS boolean
LANGUAGE sql SECURITY DEFINER VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    UPDATE syspilot_control.notification_deliveries
    SET status='delivered',delivered_at=clock_timestamp(),failure_code=NULL
    WHERE tenant_id=p_tenant_id AND delivery_id=p_delivery_id AND status='sending'
    RETURNING true
$$;

CREATE FUNCTION syspilot_control.fail_notification_delivery(
    p_tenant_id uuid,p_delivery_id bigint,p_failure_code text
) RETURNS boolean
LANGUAGE sql SECURITY DEFINER VOLATILE
SET search_path = pg_catalog, syspilot_control
AS $$
    UPDATE syspilot_control.notification_deliveries
    SET status=CASE WHEN attempt_count >= 10 THEN 'failed' ELSE 'pending' END,
        available_at=clock_timestamp() + make_interval(secs => LEAST(300,power(2,LEAST(attempt_count,8))::integer)),
        failure_code=left(p_failure_code,100)
    WHERE tenant_id=p_tenant_id AND delivery_id=p_delivery_id AND status='sending'
    RETURNING true
$$;

REVOKE ALL ON FUNCTION syspilot_control.lease_notification_delivery() FROM PUBLIC;
REVOKE ALL ON FUNCTION syspilot_control.complete_notification_delivery(uuid,bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION syspilot_control.fail_notification_delivery(uuid,bigint,text) FROM PUBLIC;
GRANT USAGE ON SCHEMA syspilot_control TO syspilot_notification_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.lease_notification_delivery() TO syspilot_notification_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.complete_notification_delivery(uuid,bigint) TO syspilot_notification_worker;
GRANT EXECUTE ON FUNCTION syspilot_control.fail_notification_delivery(uuid,bigint,text) TO syspilot_notification_worker;

COMMIT;
