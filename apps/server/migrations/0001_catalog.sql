CREATE TABLE service_accounts (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE, enabled boolean NOT NULL DEFAULT true,
    can_create_buckets boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE access_keys (
    id uuid PRIMARY KEY, account_id uuid NOT NULL REFERENCES service_accounts(id),
    access_key text NOT NULL UNIQUE, encrypted_secret bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), revoked_at timestamptz
);
CREATE INDEX access_keys_account ON access_keys(account_id);
CREATE TABLE buckets (
    id uuid PRIMARY KEY, name text NOT NULL UNIQUE, owner_id uuid NOT NULL REFERENCES service_accounts(id),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX buckets_owner ON buckets(owner_id);
CREATE TABLE roles (name text PRIMARY KEY);
INSERT INTO roles VALUES ('Admin'), ('ReadWrite'), ('ReadOnly');
CREATE TABLE policy_bindings (
    id uuid PRIMARY KEY, account_id uuid NOT NULL REFERENCES service_accounts(id),
    bucket_id uuid NOT NULL REFERENCES buckets(id) ON DELETE CASCADE, role text NOT NULL REFERENCES roles(name),
    UNIQUE(account_id,bucket_id)
);
CREATE TABLE objects (
    id uuid PRIMARY KEY, bucket_id uuid NOT NULL REFERENCES buckets(id) ON DELETE RESTRICT,
    key text COLLATE "C" NOT NULL, storage_id uuid NOT NULL UNIQUE, size bigint NOT NULL CHECK(size>=0),
    sha256 text NOT NULL, etag text NOT NULL, content_type text NOT NULL, metadata jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(bucket_id,key)
);
CREATE TABLE uploads (
    id uuid PRIMARY KEY, bucket_id uuid NOT NULL REFERENCES buckets(id) ON DELETE CASCADE,
    key text NOT NULL, account_id uuid NOT NULL REFERENCES service_accounts(id),
    content_type text NOT NULL, metadata jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX uploads_expiry ON uploads(created_at);
CREATE TABLE parts (
    upload_id uuid NOT NULL REFERENCES uploads(id) ON DELETE CASCADE,
    number integer NOT NULL CHECK(number BETWEEN 1 AND 10000), storage_id uuid NOT NULL UNIQUE,
    size bigint NOT NULL CHECK(size>=0), sha256 text NOT NULL, etag text NOT NULL,
    PRIMARY KEY(upload_id,number)
);
CREATE TABLE audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, occurred_at timestamptz NOT NULL DEFAULT now(),
    request_id uuid NOT NULL, principal_id uuid, action text NOT NULL, resource text NOT NULL,
    result text NOT NULL, source_ip inet
);
CREATE INDEX audit_time ON audit_events(occurred_at);
