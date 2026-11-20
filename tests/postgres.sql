\set ON_ERROR_STOP on
BEGIN;
-- An isolated schema exercises the shipped migrations. Rollback also covers failed runs.
SELECT 'datavault_test_' || pg_backend_pid() AS test_schema \gset
CREATE SCHEMA :"test_schema";
SET LOCAL search_path TO :"test_schema", pg_catalog;
\ir ../apps/server/migrations/0001_catalog.sql
\ir ../apps/server/migrations/0002_catalog_identity.sql

DO $test$
DECLARE
    account uuid := gen_random_uuid();
    bucket uuid := gen_random_uuid();
    original uuid := gen_random_uuid();
    second uuid := gen_random_uuid();
    page text[];
BEGIN
    INSERT INTO service_accounts (id,name) VALUES (account,'test');
    INSERT INTO buckets (id,name,owner_id) VALUES (bucket,'warehouse',account);
    BEGIN
        INSERT INTO buckets (id,name,owner_id) VALUES (gen_random_uuid(),'warehouse',account);
        RAISE EXCEPTION 'duplicate bucket accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    INSERT INTO objects (id,bucket_id,key,storage_id,size,etag,sha256,content_type,metadata)
    VALUES (gen_random_uuid(),bucket,'a/1',original,1,'e','s','text/plain','{}'),
           (gen_random_uuid(),bucket,'a/2',second,2,'e','s','text/plain','{}'),
           (gen_random_uuid(),bucket,'z',gen_random_uuid(),3,'e','s','text/plain','{}');
    BEGIN
        UPDATE objects SET storage_id=gen_random_uuid() WHERE bucket_id=bucket AND key='a/1';
        RAISE EXCEPTION USING ERRCODE='ZV001', MESSAGE='simulated failure before commit';
    EXCEPTION WHEN SQLSTATE 'ZV001' THEN NULL;
    END;
    IF (SELECT storage_id FROM objects WHERE bucket_id=bucket AND key='a/1') <> original THEN
        RAISE EXCEPTION 'rollback lost previous object';
    END IF;
    SELECT array_agg(key) INTO page FROM
        (SELECT key FROM objects WHERE bucket_id=bucket AND key LIKE 'a/%' AND key>'a/1' ORDER BY key LIMIT 1) p;
    IF page IS DISTINCT FROM ARRAY['a/2'] THEN RAISE EXCEPTION 'wrong continuation page'; END IF;
    BEGIN
        DELETE FROM buckets WHERE id=bucket;
        RAISE EXCEPTION 'nonempty bucket deleted';
    EXCEPTION WHEN foreign_key_violation THEN NULL;
    END;
    UPDATE objects SET size=4 WHERE bucket_id=bucket AND key='a/1';
    IF (SELECT size FROM objects WHERE storage_id=original) <> 4 THEN RAISE EXCEPTION 'overwrite lost'; END IF;
    DELETE FROM objects WHERE bucket_id=bucket;
    DELETE FROM buckets WHERE id=bucket;
    IF EXISTS(SELECT FROM buckets WHERE id=bucket) THEN RAISE EXCEPTION 'delete failed'; END IF;
END
$test$;
ROLLBACK;
\echo 'PostgreSQL migrations, constraints, rollback, keyset lookup, update and delete passed'
