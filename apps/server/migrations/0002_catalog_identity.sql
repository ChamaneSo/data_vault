CREATE TABLE catalog_identity (singleton boolean PRIMARY KEY CHECK(singleton), id uuid NOT NULL UNIQUE);
INSERT INTO catalog_identity VALUES (true, gen_random_uuid());
