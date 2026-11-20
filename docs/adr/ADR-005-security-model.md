# ADR 005 · Recoverable signing keys and scoped grants

SigV4 verification needs the signing secret. Each key is encrypted using AES-256-GCM with an independent random nonce and the public key identifier as associated data. The encryption key and bootstrap admin token come from deployment configuration. Service accounts, active signing keys, and grants are checked during normal operation. The bucket creator owns that bucket. Creating another bucket requires an explicit account capability.
