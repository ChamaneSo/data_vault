# DataVault 0.1.0 implementation status

The source, migrations, Compose projects, CI workflow and test fixtures are present. Docker execution belongs to the operator. This reconstruction has not yet produced a running image or a release-qualified binary.

| Milestone | Status | Evidence and outstanding acceptance check |
| --- | --- | --- |
| 1. Foundation | IN PROGRESS | Workspace, configuration, JSON logging, health probes, migrations and Compose are present. Credential generation and preservation passed a shell regression test. The first container bootstrap must generate the dependency lockfile. Compose startup awaits execution. |
| 2. Storage | IMPLEMENTED | UUID blobs, bounded input/output, checksum checks, file and directory sync, single-writer lock, volume identity, startup checks and offline archival. Rust tests cover ranges, interruption, subprocess termination during a write, missing files, tampering, and symlink rejection. Execution pending. |
| 3. Metadata | IMPLEMENTED | PostgreSQL schema, transactional publication, scoped advisory locks, row-level permission checks and indexed C-collated keyset listing. A SQL regression fixture runs both migrations in an isolated transaction. Execution pending. |
| 4. Core S3 | IMPLEMENTED | Bucket and object CRUD, HEAD, prefix/delimiter listing, metadata and byte-range GET. Wire tests await the first image build. |
| 5. Authentication and authorization | IMPLEMENTED | SigV4, scoped roles, key issuance and revocation, account disabling, AES-GCM secrets and TLS configuration. Live authorization and denial tests pending. |
| 6. Administration and CLI | IMPLEMENTED | Versioned JSON routes, one-time secret issuance, Clap commands and OpenAPI. Static checks matched all 11 documented paths to the router. Container CLI calls pending. |
| 7. Expanded compatibility | IN PROGRESS | Five multipart operations, copy, presigned URLs and ETag conditions are present. Interoperability tests remain unexecuted. Date conditions and full checksum negotiation are outside current coverage. |
| 8. Hardening and validation | IN PROGRESS | CI, observability, isolated fault fixtures, AWS CLI restart demonstration, benchmark and six decision records are present. Compilation, linting, dependency audit, failure injection and end-to-end tests await container execution. |

**IMPLEMENTED** means the code exists. **TESTED** requires executing the relevant acceptance test. No full milestone has been marked TESTED.

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

The Cargo build, formatting gate, unit tests, SQL migrations, container health checks, PostgreSQL transactions, AWS compatibility tests, CI and restart demonstration still require execution. No runtime pass/fail count or performance measurement is available. The subprocess storage test and deferred-commit fixture cover specific failure points. A full-server kill during an HTTP transfer and a live database disconnect remain separate operational acceptance checks.

Offline recovery preserves unused bytes under a recovery directory. Operators control archive retention and can abort individual multipart uploads. There is no automatic expiration of multipart sessions. Disk use can exceed reported logical bytes. Multi-node replication, failover, object history, and a tamper-evident audit trail are outside this version.
