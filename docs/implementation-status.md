# DataVault 0.1.0 implementation status

The operator's Docker run reached a healthy server. Eight integration tests passed and one failed: multipart completion returned `EntityTooSmall` for a reversed part list. The handler now checks the non-final size limit after validating the manifest. The revised server and integration test await a rerun.

| Milestone | Status | Evidence and outstanding acceptance check |
| --- | --- | --- |
| 1. Foundation | TESTED | Compose reported a healthy server and the integration suite performed real PostgreSQL-backed operations. |
| 2. Storage | IMPLEMENTED | UUID blobs, bounded input/output, checksum checks, file and directory sync, single-writer lock, volume identity, startup checks and offline archival. Rust tests cover ranges, interruption, subprocess termination during a write, missing files, tampering, and symlink rejection. Execution pending. |
| 3. Metadata | IMPLEMENTED | PostgreSQL schema, transactional publication, scoped advisory locks, row-level permission checks and indexed C-collated keyset listing. A SQL regression fixture runs both migrations in an isolated transaction. Execution pending. |
| 4. Core S3 | TESTED | The core object, listing, and AWS CLI wire tests passed in the reported run. |
| 5. Authentication and authorization | IMPLEMENTED | Live signing, scoped permissions and denial cases passed in the reported run. TLS deployment validation is outstanding. |
| 6. Administration and CLI | IMPLEMENTED | Versioned JSON routes, one-time secret issuance, Clap commands and OpenAPI. Static checks matched all 11 documented paths to the router. Container CLI calls pending. |
| 7. Expanded compatibility | IN PROGRESS | Copy and presigned URL cases passed. Multipart order validation has been corrected in source. Successful completion, retry and abort await the rerun. |
| 8. Hardening and validation | IN PROGRESS | Eight integration cases passed before the multipart fix. Fault injection, audit, CI and restart results are outstanding. |

**IMPLEMENTED** means the code exists. **TESTED** means a relevant acceptance test passed.

## Checks executed during reconstruction

- Bootstrap script syntax and a fixture-based regression passed using the already available Git Bash. It generated the expected secret formats and preserved existing configuration and a pre-existing fixture lockfile on a second run. Dependency resolution was outside that test.
- OpenAPI JSON parsed successfully. All 11 endpoint paths mapped to the Rust admin and monitoring router.
- Checked the 15 S3 request/response type pairs and 21 referenced protocol error codes against the pinned adapter source.
- Verified that the temporary host compiler, installer and downloads had been removed.
- Checked Rust module file references, local Markdown links, loopback port declarations and the isolated fault-test volume configuration. All shell scripts passed Bash syntax checks.

## First container validation

Run these from the repository root. The bootstrap and formatter create the real lockfile and formatted working copy. Keep their changes with the source before checking CI.

```sh
docker compose -f compose.bootstrap.yml run --rm initialize
docker compose run --rm --build rust cargo fmt --all
docker compose run --rm rust cargo fmt --all --check
docker compose run --rm rust cargo clippy --locked --workspace --all-targets --all-features
docker compose run --rm rust cargo test --locked --workspace
docker compose up -d --build --wait
docker compose run --rm postgres-tests
docker compose run --rm --build integration-tests
docker compose -f compose.recovery.yml run --rm --build fault-tests
docker compose -f compose.recovery.yml down
```

The [quick start](../README.md) includes the two-phase AWS CLI demonstration. The [operations guide](operations.md) describes TLS deployment, key rotation, monitoring, offline recovery and performance measurements.

## Release gaps

Rebuild the server and rerun the integration suite to verify the multipart fix. Results for formatting, unit tests, dedicated PostgreSQL and fault suites, CI, restart and performance measurements have not been reported. A full-server kill during an HTTP transfer and a live database disconnect remain separate operational acceptance checks.

Offline recovery preserves unused bytes under a recovery directory. Operators control archive retention and can abort individual multipart uploads. There is no automatic expiration of multipart sessions. Disk use can exceed reported logical bytes. Multi-node replication, failover, object history, and a tamper-evident audit trail are outside this version.
