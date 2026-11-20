//! PostgreSQL transactions and bucket-level authorization for S3 use cases.
use crate::App;
use chrono::{DateTime, Utc};
use datavault_core::{Blob, Error, Permission, Role};
use s3s::{
    S3Error, S3Request, S3Result,
    dto::{ETag, ETagCondition, Metadata, Timestamp},
    s3_error,
};
use sqlx::{Postgres, Transaction};
use std::time::SystemTime;
use uuid::Uuid;

pub type Tx = Transaction<'static, Postgres>;

pub fn db_error(e: sqlx::Error) -> S3Error {
    tracing::error!(sqlstate=?e.as_database_error().and_then(|e| e.code()), "database operation failed");
    s3_error!(ServiceUnavailable)
}

pub fn domain(e: Error) -> S3Error {
    match e {
        Error::Invalid => s3_error!(InvalidArgument),
        Error::Denied => s3_error!(AccessDenied),
        Error::BucketExists => s3_error!(BucketAlreadyExists),
        Error::NoBucket => s3_error!(NoSuchBucket),
        Error::BucketNotEmpty => s3_error!(BucketNotEmpty),
        Error::NoObject => s3_error!(NoSuchKey),
        Error::Checksum => s3_error!(BadDigest),
        Error::Range => s3_error!(InvalidRange),
        Error::TooLarge => s3_error!(EntityTooLarge),
        Error::Precondition => s3_error!(PreconditionFailed),
        Error::Storage | Error::Database => s3_error!(ServiceUnavailable),
    }
}

#[derive(sqlx::FromRow)]
pub struct Stored {
    pub key: String,
    pub storage_id: Uuid,
    pub size: i64,
    pub etag: String,
    pub sha256: String,
    pub content_type: String,
    pub metadata: sqlx::types::Json<Metadata>,
    pub updated_at: DateTime<Utc>,
}
impl Stored {
    pub fn blob(&self) -> Blob {
        Blob {
            id: self.storage_id,
            size: self.size,
            etag: self.etag.clone(),
            sha256: self.sha256.clone(),
        }
    }
    pub fn timestamp(&self) -> Timestamp {
        Timestamp::from(SystemTime::from(self.updated_at))
    }
}

pub async fn identity<T>(app: &App, req: &S3Request<T>) -> S3Result<(Tx, Uuid, bool)> {
    let key = &req
        .credentials
        .as_ref()
        .ok_or_else(|| s3_error!(AccessDenied))?
        .access_key;
    let mut metadata_bytes = 0;
    for (name, value) in &req.headers {
        let name = name.as_str();
        if name.starts_with("x-amz-meta-") {
            metadata_bytes += name.len() + value.as_bytes().len();
        }
        if name.starts_with("x-amz-grant-")
            || name.starts_with("x-amz-server-side-encryption")
            || name.starts_with("x-amz-copy-source-server-side-encryption")
            || name.starts_with("x-amz-object-lock-")
            || matches!(
                name,
                "x-amz-acl"
                    | "x-amz-tagging"
                    | "x-amz-tagging-directive"
                    | "x-amz-expected-bucket-owner"
                    | "x-amz-source-expected-bucket-owner"
                    | "x-amz-website-redirect-location"
                    | "if-modified-since"
                    | "if-unmodified-since"
                    | "x-amz-copy-source-if-modified-since"
                    | "x-amz-copy-source-if-unmodified-since"
            )
        {
            return Err(s3_error!(NotImplemented));
        }
    }
    if metadata_bytes > 2048 {
        return Err(s3_error!(MetadataTooLarge));
    }
    let mut tx = app.db.begin().await.map_err(db_error)?;
    let (account, can_create) = sqlx::query_as("SELECT a.id,a.can_create_buckets FROM access_keys k JOIN service_accounts a ON a.id=k.account_id WHERE k.access_key=$1 AND a.enabled AND k.revoked_at IS NULL FOR SHARE OF k,a")
        .bind(key).fetch_optional(&mut *tx).await.map_err(db_error)?.ok_or_else(|| s3_error!(AccessDenied))?;
    if let Some(ctx) = req.extensions.get::<crate::RequestContext>() {
        ctx.set_principal(account);
    }
    Ok((tx, account, can_create))
}

pub async fn authorize(
    tx: &mut Tx,
    account: Uuid,
    name: &str,
    action: Permission,
) -> S3Result<Uuid> {
    let q = if matches!(action, Permission::Manage) {
        "SELECT id,owner_id FROM buckets WHERE name=$1 FOR UPDATE"
    } else {
        "SELECT id,owner_id FROM buckets WHERE name=$1 FOR SHARE"
    };
    let (id, owner): (Uuid, Uuid) = sqlx::query_as(q)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_error)?
        .ok_or_else(|| s3_error!(NoSuchBucket))?;
    if owner != account {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM policy_bindings WHERE account_id=$1 AND bucket_id=$2 FOR SHARE",
        )
        .bind(account)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_error)?;
        let role = match role.as_deref() {
            Some("Admin") => Role::Admin,
            Some("ReadWrite") => Role::ReadWrite,
            Some("ReadOnly") => Role::ReadOnly,
            _ => return Err(s3_error!(AccessDenied)),
        };
        if !role.permits(action) {
            return Err(s3_error!(AccessDenied));
        }
    }
    Ok(id)
}
pub async fn scope<T>(
    app: &App,
    req: &S3Request<T>,
    name: &str,
    action: Permission,
) -> S3Result<(Tx, Uuid, Uuid)> {
    let (mut tx, account, _) = identity(app, req).await?;
    let id = authorize(&mut tx, account, name, action).await?;
    Ok((tx, id, account))
}
pub async fn lock(tx: &mut Tx, bucket: Uuid, key: &str) -> S3Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("{bucket}/{key}"))
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(())
}
pub async fn get(tx: &mut Tx, bucket: Uuid, key: &str) -> S3Result<Stored> {
    sqlx::query_as("SELECT * FROM objects WHERE bucket_id=$1 AND key=$2")
        .bind(bucket)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_error)?
        .ok_or_else(|| s3_error!(NoSuchKey))
}
pub fn conditions(
    tag: Option<&str>,
    equals: Option<&ETagCondition>,
    differs: Option<&ETagCondition>,
    read: bool,
) -> S3Result<()> {
    let matches = |c: &ETagCondition, weak: bool| match c {
        ETagCondition::Any => tag.is_some(),
        ETagCondition::ETag(ETag::Strong(value)) => Some(value.as_str()) == tag,
        ETagCondition::ETag(ETag::Weak(value)) => weak && Some(value.as_str()) == tag,
    };
    if equals.is_some_and(|c| !matches(c, false)) {
        return Err(s3_error!(PreconditionFailed));
    }
    if differs.is_some_and(|c| matches(c, read)) {
        return Err(if read {
            s3_error!(NotModified)
        } else {
            s3_error!(PreconditionFailed)
        });
    }
    Ok(())
}
pub async fn check_write(
    tx: &mut Tx,
    bucket: Uuid,
    key: &str,
    equals: Option<&ETagCondition>,
    differs: Option<&ETagCondition>,
) -> S3Result<()> {
    lock(tx, bucket, key).await?;
    if equals.is_some() || differs.is_some() {
        let tag: Option<String> =
            sqlx::query_scalar("SELECT etag FROM objects WHERE bucket_id=$1 AND key=$2")
                .bind(bucket)
                .bind(key)
                .fetch_optional(&mut **tx)
                .await
                .map_err(db_error)?;
        conditions(tag.as_deref(), equals, differs, false)?;
    }
    Ok(())
}
pub async fn publish(
    tx: &mut Tx,
    bucket: Uuid,
    key: &str,
    blob: &Blob,
    content_type: &str,
    metadata: Metadata,
) -> S3Result<()> {
    sqlx::query(
        "INSERT INTO objects(id,bucket_id,key,storage_id,size,etag,sha256,content_type,metadata)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(bucket_id,key) DO UPDATE SET
        storage_id=excluded.storage_id,size=excluded.size,etag=excluded.etag,sha256=excluded.sha256,
        content_type=excluded.content_type,metadata=excluded.metadata,updated_at=clock_timestamp()",
    )
    .bind(Uuid::new_v4())
    .bind(bucket)
    .bind(key)
    .bind(blob.id)
    .bind(blob.size)
    .bind(&blob.etag)
    .bind(&blob.sha256)
    .bind(content_type)
    .bind(sqlx::types::Json(metadata))
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}
