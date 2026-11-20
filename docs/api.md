# API reference

## S3

Send path-style requests signed for service `s3` and the configured region, initially `us-east-1`. Bucket names are globally unique within the deployment. New service accounts require an explicit creation grant to create buckets.

| Resource | Operations and behavior |
| --- | --- |
| Buckets | CreateBucket, ListBuckets filtered by account, HeadBucket, and DeleteBucket with nonempty detection |
| Objects | PutObject, GetObject, HeadObject, DeleteObject, and CopyObject with independent source and destination authorization |
| Listing | ListObjectsV2 with prefix, delimiter, max-keys, start-after, and opaque continuation tokens |
| Metadata | Content type, user metadata, quoted ETag and modification time |
| Conditions | ETag-based If-Match and If-None-Match on implemented reads and writes. Single byte-range GET. |
| Multipart | CreateMultipartUpload, UploadPart, ListParts, CompleteMultipartUpload, AbortMultipartUpload. Session identifiers are bound to account, bucket and key. |
| Integrity | SHA-256 internally. Content-MD5, SHA-256, CRC32, CRC32C, and CRC64NVME on upload where provided. GET verifies stored bytes before returning a body. |

The supported deployment uses private, authenticated buckets. ACLs, bucket policies, SSE, object version selection, HTTP POST forms, bucket notifications, object locking, and distributed operations are outside the implemented feature set. Timestamp-based conditional requests, the complete AWS checksum negotiation matrix, and compatibility with all AWS SDK versions have not been validated. S3 errors use XML and a mapped HTTP status. The response carries an `x-amz-request-id` for log correlation.

## Administration

Use `Authorization: Bearer <administrator token>` and JSON on the dedicated admin listener. A successful key-creation response is the only API response carrying its signing secret. Deleting an account disables it and preserves audit and ownership records. Granting a new role for an existing account/bucket pair replaces that role.

The machine-readable schema lists paths, request fields, return values, and security requirements. [OpenAPI specification](api/openapi.json)

Health checks on `/health/live` and `/health/ready` and the optional `/metrics` endpoint reside on the admin listener. Readiness queries PostgreSQL, publishes a one-byte filesystem probe, and reclaims it. The metrics scrape reports logical bytes and object counts, request count and duration, failures, and upload/download bytes. Download bytes count scheduled response lengths. Interrupted responses can make this higher than bytes received by clients.
