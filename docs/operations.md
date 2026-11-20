# Deployment and development

## Configuration

The bootstrap Compose project creates development-only random secrets and a dependency lockfile in the project. Environment variables are documented in `.env.example`. Compose loads the resulting `.env`. A standalone server may read dotenv settings from `DATAVAULT_CONFIG`. Explicit environment values take priority. DataVault requires a long administrator token, a base64-encoded 32-byte encryption key, and a PostgreSQL password.

The default Compose deployment explicitly selects development HTTP and publishes both listeners on loopback. For production configuration, set `DATAVAULT_ENV=production`, mount a PEM certificate chain and private key, and set `DATAVAULT_TLS_CERT` and `DATAVAULT_TLS_KEY` to their mounted paths. Configure PostgreSQL for certificate-verified TLS, with `DATAVAULT_DATABASE_CA` pointing to its CA file if needed. Adapt the Compose HTTP readiness URL, client endpoints, and Prometheus scheme to HTTPS and the correct trust store. This repository supplies a development Compose stack.

Rotate an S3 signing key by issuing a second key, updating clients, verifying access, and revoking the old key by UUID. Rotate the admin token through the deployment secret configuration and recreate the server container. An authorized host operator can restore admin access through the same configuration. Preserve the master encryption key across rebuilds and backups. Changing it without re-encrypting the catalog makes existing signing secrets unreadable.

## Development

Modify the core for shared type and permission invariants, the server modules for transport, storage or transaction behavior, and the CLI for administrator workflows. Create a new numbered SQL migration for schema changes. SQLx embeds migrations at compile time and applies them at startup. Rebuild the server image after Rust or migration changes. Keep secrets and data volumes outside the build context.

The optional Makefile calls the same Compose commands as the quick start. All commands also work directly from PowerShell through Docker Compose. The Rust tools image contains Cargo, rustfmt, and Clippy. Its `/build` target and registry cache use named volumes. The first formatting run updates the working tree. Keep `Cargo.lock` after bootstrap and review dependency updates in source control.

## Validation

The test image starts only after the service becomes healthy. `docker compose run --rm --build integration-tests` returns the pytest exit code. Coverage includes real SigV4 requests, metadata and range reads, overwrite races, prefix pagination, delimiter grouping, copy, default denial, read-only bindings, revocation, invalid signatures, multipart completion and abort, and an AWS CLI round trip. The two demo phases in the README check persistence across a full container restart. All test identities and buckets use generated names.

For performance measurements, run the following in a development deployment. Record CPU, memory limit, storage filesystem, Docker version, PostgreSQL settings, object size, concurrency and whether the page cache was warm. The benchmark prints median request times, bidirectional throughput, and listing latency. Sample server memory with Docker stats while it runs.

```sh
docker compose run --rm integration-tests python benchmark.py --objects 40 --workers 4 --mib 8
docker stats --no-stream
```

## Recovery exercise

To isolate unused files, stop the service, run the maintenance process and restart the service. PostgreSQL stays online. The command takes both ownership locks, checks every referenced UUID, then moves unreferenced files and interrupted staging uploads into a fresh directory under `/var/lib/datavault/recovery`. It preserves their bytes and their original relative names. The archive uses space on the same volume. Completed objects and parts belonging to ongoing uploads stay in place.

```sh
docker compose stop datavault
docker compose run --rm datavault archive-unused
docker compose up -d --wait
```

Stop the writer before taking matching backups of PostgreSQL, the object volume, and the encryption key. Test restores into an isolated deployment. An interrupted upload must never become visible under its logical key. A missing referenced UUID prevents startup. A checksum mismatch during GET results in an error response. Startup logs report unreferenced-file and staging counts. Preserve those files during diagnosis and reconcile against backup history.

The dedicated recovery Compose project mounts a 16 MiB scratch filesystem and uses a separate temporary PostgreSQL catalog. One test exhausts that scratch filesystem during an overwrite. Another uses a deferred constraint trigger to reject the SQL commit after the replacement blob has been written. Both tests check that the client receives an error and the previously acknowledged object remains readable. Run these after bootstrap.

```sh
docker compose -f compose.recovery.yml run --rm --build fault-tests
docker compose -f compose.recovery.yml down
```

The regular PostgreSQL test service applies both migrations in an isolated schema within one rolled-back transaction. Run it with `docker compose run --rm postgres-tests`. The filesystem tests exercise interrupted input, bounded streaming, missing references, symlinks, restart, corruption, and offline archival. A subprocess test stops a storage writer after its first chunk reaches staging and verifies the previously committed blob on reopen. Abrupt termination of the whole HTTP server and loss of the live PostgreSQL connection still require operator validation.
