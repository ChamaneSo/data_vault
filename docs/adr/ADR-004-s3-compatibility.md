# ADR 004 · Use an S3 protocol adapter

The S3 listener uses the s3s adapter for SigV4 canonicalization, request decoding and XML serialization. Handlers implement operations against the local catalog and storage port. Integration tests drive the wire protocol through boto3 and the AWS CLI. The service publishes a supported operation subset. Administrative JSON has its own versioned endpoint and credentials.
