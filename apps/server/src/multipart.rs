//! Multipart sessions and parts stay invisible to GET and object listing until commit.
use crate::{
    App, body,
    catalog::{self, Tx, db_error, domain},
};
use datavault_core::{Blob, Permission, object_key};
use futures_util::StreamExt;
use md5::{Digest, Md5};
use s3s::{S3Request, S3Response, S3Result, dto::*, s3_error};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct Session {
    id: Uuid,
    content_type: String,
    metadata: sqlx::types::Json<Metadata>,
}

async fn session(
    tx: &mut Tx,
    id: &str,
    account: Uuid,
    bucket: Uuid,
    key: &str,
) -> S3Result<Session> {
    let id = Uuid::parse_str(id).map_err(|_| s3_error!(NoSuchUpload))?;
    sqlx::query_as("SELECT id,content_type,metadata FROM uploads WHERE id=$1 AND account_id=$2 AND bucket_id=$3 AND key=$4 FOR UPDATE")
        .bind(id).bind(account).bind(bucket).bind(key).fetch_optional(&mut **tx).await.map_err(db_error)?
        .ok_or_else(|| s3_error!(NoSuchUpload))
}
pub async fn create(
    app: &App,
    req: S3Request<CreateMultipartUploadInput>,
) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
    let (mut tx, bucket, account) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Write).await?;
    let i = req.input;
    object_key(&i.key).map_err(domain)?;
    if i.acl.is_some() || i.server_side_encryption.is_some() {
        return Err(s3_error!(NotImplemented));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO uploads(id,account_id,bucket_id,key,content_type,metadata) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(account).bind(bucket).bind(&i.key).bind(i.content_type.as_deref().unwrap_or("application/octet-stream"))
        .bind(sqlx::types::Json(i.metadata.unwrap_or_default())).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(S3Response::new(CreateMultipartUploadOutput {
        bucket: Some(i.bucket),
        key: Some(i.key),
        upload_id: Some(id.to_string()),
        ..Default::default()
    }))
}
pub async fn part(
    app: &App,
    req: S3Request<UploadPartInput>,
) -> S3Result<S3Response<UploadPartOutput>> {
    let (mut tx, bucket, account) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Write).await?;
    let i = req.input;
    if !(1..=10000).contains(&i.part_number) {
        return Err(s3_error!(InvalidArgument));
    }
    let upload = session(&mut tx, &i.upload_id, account, bucket, &i.key).await?;
    let blob = body::upload(app, &req.headers, i.body, i.content_length, || {
        req.trailing_headers.as_ref().and_then(|t| t.take())
    })
    .await?;
    sqlx::query("INSERT INTO parts(upload_id,number,storage_id,size,sha256,etag) VALUES($1,$2,$3,$4,$5,$6)
        ON CONFLICT(upload_id,number) DO UPDATE SET storage_id=excluded.storage_id,size=excluded.size,sha256=excluded.sha256,etag=excluded.etag")
        .bind(upload.id).bind(i.part_number).bind(blob.id).bind(blob.size).bind(blob.sha256).bind(&blob.etag)
        .execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(S3Response::new(UploadPartOutput {
        e_tag: Some(ETag::Strong(blob.etag)),
        checksum_crc32: i.checksum_crc32,
        checksum_sha256: i.checksum_sha256,
        ..Default::default()
    }))
}
pub async fn list(
    app: &App,
    req: S3Request<ListPartsInput>,
) -> S3Result<S3Response<ListPartsOutput>> {
    let (mut tx, bucket, account) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Read).await?;
    let i = req.input;
    let upload = session(&mut tx, &i.upload_id, account, bucket, &i.key).await?;
    let marker = i.part_number_marker.unwrap_or(0);
    let limit = i.max_parts.unwrap_or(1000);
    if !(0..=10000).contains(&marker) || !(0..=1000).contains(&limit) {
        return Err(s3_error!(InvalidArgument));
    }
    let rows:Vec<(i32,i64,String)>=sqlx::query_as("SELECT number,size,etag FROM parts WHERE upload_id=$1 AND number>$2 ORDER BY number LIMIT $3")
        .bind(upload.id).bind(marker).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(db_error)?;
    let truncated = limit > 0 && rows.len() > limit as usize;
    let next = truncated.then(|| rows[limit as usize - 1].0);
    let parts = rows
        .into_iter()
        .take(limit as usize)
        .map(|(n, size, tag)| Part {
            part_number: Some(n),
            size: Some(size),
            e_tag: Some(ETag::Strong(tag)),
            ..Default::default()
        })
        .collect();
    Ok(S3Response::new(ListPartsOutput {
        bucket: Some(i.bucket),
        key: Some(i.key),
        upload_id: Some(i.upload_id),
        parts: Some(parts),
        max_parts: Some(limit),
        part_number_marker: Some(marker),
        is_truncated: Some(truncated),
        next_part_number_marker: next,
        ..Default::default()
    }))
}
pub async fn complete(
    app: &App,
    req: S3Request<CompleteMultipartUploadInput>,
) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
    let (mut tx, bucket, account) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Write).await?;
    let i = req.input;
    let upload = session(&mut tx, &i.upload_id, account, bucket, &i.key).await?;
    let parts = i
        .multipart_upload
        .and_then(|v| v.parts)
        .ok_or_else(|| s3_error!(InvalidPart))?;
    if parts.is_empty() || parts.len() > 10000 {
        return Err(s3_error!(InvalidPart));
    }
    catalog::check_write(
        &mut tx,
        bucket,
        &i.key,
        i.if_match.as_ref(),
        i.if_none_match.as_ref(),
    )
    .await?;
    let rows: Vec<(i32, Uuid, i64, String, String)> = sqlx::query_as(
        "SELECT number,storage_id,size,sha256,etag FROM parts WHERE upload_id=$1 ORDER BY number",
    )
    .bind(upload.id)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_error)?;
    let mut selected = Vec::with_capacity(parts.len());
    let mut previous = 0;
    let mut digest = Md5::new();
    let count = parts.len();
    for p in parts {
        let n = p.part_number.ok_or_else(|| s3_error!(InvalidPart))?;
        if n <= previous {
            return Err(s3_error!(InvalidPartOrder));
        }
        let idx = rows
            .binary_search_by_key(&n, |r| r.0)
            .map_err(|_| s3_error!(InvalidPart))?;
        let (_, id, size, sha, tag) = &rows[idx];
        if p.e_tag.as_ref().and_then(ETag::as_strong) != Some(tag.as_str()) {
            return Err(s3_error!(InvalidPart));
        }
        digest.update(hex::decode(tag).map_err(|_| s3_error!(InternalError))?);
        selected.push(Blob {
            id: *id,
            size: *size,
            etag: tag.clone(),
            sha256: sha.clone(),
        });
        previous = n;
    }
    // Only the final part of a valid, ordered manifest may be smaller than 5 MiB.
    if selected[..count - 1]
        .iter()
        .any(|part| part.size < 5 * 1024 * 1024)
    {
        return Err(s3_error!(EntityTooSmall));
    }
    let storage = app.storage.clone();
    let stream = async_stream::try_stream! {
        for part in selected {
            storage.verify(&part).await.map_err(std::io::Error::other)?;
            let mut data=storage.read(&part,0,part.size as u64).await.map_err(std::io::Error::other)?;
            while let Some(chunk)=data.next().await {yield chunk?;}
        }
    };
    let mut blob = app
        .storage
        .put(Box::pin(stream), body::MAX_OBJECT)
        .await
        .map_err(domain)?;
    blob.etag = format!("{}-{count}", hex::encode(digest.finalize()));
    catalog::publish(
        &mut tx,
        bucket,
        &i.key,
        &blob,
        &upload.content_type,
        upload.metadata.0,
    )
    .await?;
    sqlx::query("DELETE FROM uploads WHERE id=$1")
        .bind(upload.id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(S3Response::new(CompleteMultipartUploadOutput {
        bucket: Some(i.bucket),
        key: Some(i.key),
        e_tag: Some(ETag::Strong(blob.etag)),
        ..Default::default()
    }))
}
pub async fn abort(
    app: &App,
    req: S3Request<AbortMultipartUploadInput>,
) -> S3Result<S3Response<AbortMultipartUploadOutput>> {
    let (mut tx, bucket, account) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Write).await?;
    let i = req.input;
    let upload = session(&mut tx, &i.upload_id, account, bucket, &i.key).await?;
    sqlx::query("DELETE FROM uploads WHERE id=$1")
        .bind(upload.id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(S3Response::new(AbortMultipartUploadOutput::default()))
}
