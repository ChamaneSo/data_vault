# ADR 002 · PostgreSQL owns the logical namespace

SQL transactions coordinate account status, role bindings, bucket lifecycle, and the current object reference. Foreign keys protect referenced rows. A unique C-collated bucket/key index supports ordered listing and prefix scans. A distinct UUID identifies each physical revision. SQLx embeds the migration files into the server. PostgreSQL and its WAL must reside on durable storage.
