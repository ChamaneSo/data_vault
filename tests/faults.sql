-- Only the isolated recovery Compose project uses this deferred failure fixture.
DO $guard$
BEGIN
    IF current_database() <> 'datavault_recovery' THEN
        RAISE EXCEPTION 'Recovery fixture requires the isolated recovery database';
    END IF;
END
$guard$;

CREATE OR REPLACE FUNCTION reject_test_publication() RETURNS trigger LANGUAGE plpgsql AS $function$
BEGIN
    IF NEW.key='commit-failure' AND NEW.metadata->>'test'='reject' THEN
        RAISE EXCEPTION 'Injected commit failure';
    END IF;
    RETURN NULL;
END
$function$;

DO $install$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_trigger
        WHERE tgname='datavault_reject_publication' AND tgrelid='objects'::regclass) THEN
        CREATE CONSTRAINT TRIGGER datavault_reject_publication
        AFTER INSERT OR UPDATE ON objects DEFERRABLE INITIALLY DEFERRED
        FOR EACH ROW EXECUTE FUNCTION reject_test_publication();
    END IF;
END
$install$;
