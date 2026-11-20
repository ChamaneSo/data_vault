# DataVault

DataVault is a single-node Rust object store with a path-style S3 endpoint, PostgreSQL catalog, and a separate administrator API. Object bytes live in a Docker volume under opaque UUID filenames. Applications use ordinary AWS credentials. Administrators issue and revoke those credentials over an authenticated HTTP interface.

## Start on Windows, macOS, or Linux

Docker with Compose is the only host prerequisite. Run these commands from the repository root.

```sh
docker compose -f compose.bootstrap.yml run --rm initialize
docker compose up -d --build --wait
docker compose ps
docker compose run --rm --build cli server status
```

The bootstrap container creates a private development `.env` and resolves `Cargo.lock` on its first run. Later runs preserve both files. Keep the generated lockfile in version control. The server opens the catalog, applies embedded SQLx migrations, checks the data volume, and starts listening. S3 is at `http://localhost:9000`. Administration is at `http://localhost:9001/api/v1`. Both host ports bind to loopback.

## Create an application identity

```sh
docker compose run --rm cli user create warehouse --can-create-buckets
docker compose run --rm cli access-key create ACCOUNT_UUID
```

Use the UUID returned by the first command. The second returns a public key identifier and a one-time signing secret. Supply them as `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` to the client process. The Compose CLI service forwards those environment variables. Each account can own multiple keys for rotation.

```sh
docker compose run --rm cli bucket create warehouse-data
docker compose run --rm cli object upload README.md warehouse-data examples/readme.md
docker compose run --rm cli object list warehouse-data
docker compose run --rm cli object download warehouse-data examples/readme.md downloaded.md
docker compose run --rm cli storage stats
```

The CLI container mounts the project at `/work` for the upload and download commands. Grant another account access to one bucket with `docker compose run --rm cli permission grant ACCOUNT_UUID ReadOnly warehouse-data`. Accounts start without access to other accounts’ buckets. Bucket owners can administer their own buckets.

## Try the AWS CLI and boto3

The test image includes both clients. These commands create a unique demonstration bucket, upload and list a file with the AWS CLI, download it, compare its SHA-256 digest, then repeat the read after recreating the platform.

```sh
docker compose run --rm --build integration-tests python demo.py write
docker compose down
docker compose up -d --wait
docker compose run --rm integration-tests python demo.py verify
```

The demo keeps its temporary credentials in a private named volume. Verification removes the demo object and bucket and revokes the credentials. The standard AWS CLI is also available directly inside the CLI image.

```sh
docker compose run --rm --entrypoint aws cli --endpoint-url http://datavault:9000 s3 ls
```

A boto3 application on the Compose network can use the following code with its issued credentials in the environment.

```python
import boto3
from botocore.config import Config

store = boto3.client("s3", endpoint_url="http://datavault:9000",
                     region_name="us-east-1",
                     config=Config(s3={"addressing_style": "path"}))
store.put_object(Bucket="warehouse-data", Key="example/hello.txt", Body=b"Hello DataVault")
assert store.get_object(Bucket="warehouse-data", Key="example/hello.txt")["Body"].read() == b"Hello DataVault"
```

## Validate in containers

```sh
docker compose run --rm --build rust cargo fmt --all
docker compose run --rm rust cargo fmt --all --check
docker compose run --rm rust cargo clippy --locked --workspace --all-targets --all-features
docker compose run --rm rust cargo test --locked --workspace
docker compose run --rm postgres-tests
docker compose run --rm --build integration-tests
docker compose run --rm integration-tests pytest -q -k aws_cli
docker compose run --rm --build audit
```

The first command formats the working tree. Rust compilation, crate downloads, Python packages, and test execution all run in containers. Build caches use named volumes.

## Operate

```sh
docker compose logs -f datavault
docker compose --profile monitoring up -d
docker compose down
```

The named volumes `datavault-data` and `postgres-data` survive a normal shutdown. **`docker compose down -v` deletes development data and the catalog.** The optional `make clean` shortcut requires an explicit confirmation value. Back up PostgreSQL, the corresponding filesystem volume, and the master encryption key together.

## Implementation and limits

The server contains S3 bucket and object handlers, ListObjectsV2 pagination and delimiter grouping, ETag conditions, range GET, multipart sessions, CopyObject, and SigV4 verification. Administration includes account and key lifecycle, bucket grants, health checks, and Prometheus metrics. The source and container test suite have been reconstructed. Docker compilation, interoperability checks and restart validation await execution. This release has no replication, failover, version history, or capacity guarantee. Startup reports unused files for recovery review. Repeated overwrites can increase disk consumption.

- [Architecture and persistence](docs/architecture.md)
- [S3 compatibility and administration](docs/api.md)
- [Admin OpenAPI specification](docs/api/openapi.json)
- [Deployment, maintenance and testing](docs/operations.md)
- [Implementation and validation status](docs/implementation-status.md)
