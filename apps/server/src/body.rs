use crate::{App, catalog::domain};
use axum::http::HeaderMap;
use base64::{Engine, engine::general_purpose::STANDARD};
use datavault_core::Blob;
use futures_util::TryStreamExt;
use s3s::{S3Result, dto::StreamingBlob, s3_error};
use std::{io, sync::atomic::Ordering};

pub const MAX_PART: u64 = 5 * 1024 * 1024 * 1024;
pub const MAX_OBJECT: u64 = 5 * 1024 * 1024 * 1024 * 1024;

const CRC32C: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
const CRC64: crc::Crc<u64> = crc::Crc::<u64>::new(&crc::CRC_64_NVME);
struct Digests {
    crc32: crc32fast::Hasher,
    crc32c: crc::Digest<'static, u32>,
    crc64: crc::Digest<'static, u64>,
}
impl Digests {
    fn new() -> Self {
        Self {
            crc32: crc32fast::Hasher::new(),
            crc32c: CRC32C.digest(),
            crc64: CRC64.digest(),
        }
    }
}

pub async fn upload(
    app: &App,
    headers: &HeaderMap,
    body: Option<StreamingBlob>,
    length: Option<i64>,
    trailers: impl FnOnce() -> Option<HeaderMap> + Send,
) -> S3Result<Blob> {
    if length.is_some_and(|n| n < 0 || n as u64 > MAX_PART) {
        return Err(s3_error!(EntityTooLarge));
    }
    let body = body.unwrap_or_else(|| StreamingBlob::from_bytes(axum::body::Bytes::new()));
    let digest = std::sync::Arc::new(std::sync::Mutex::new(Digests::new()));
    let output = digest.clone();
    let stream = body
        .map_ok(move |chunk| {
            if let Ok(mut d) = output.lock() {
                d.crc32.update(&chunk);
                d.crc32c.update(&chunk);
                d.crc64.update(&chunk);
            }
            chunk
        })
        .map_err(io::Error::other);
    let blob = app
        .storage
        .put(Box::pin(stream), MAX_PART)
        .await
        .map_err(domain)?;
    let trailer_headers = trailers().unwrap_or_default();
    let announced = headers
        .get("x-amz-trailer")
        .map(|h| h.to_str().unwrap_or(""));
    let has_trailers = announced.is_none_or(|names| {
        names
            .split(',')
            .all(|name| trailer_headers.contains_key(name.trim()))
    });
    let decoded_length = headers
        .get("x-amz-decoded-content-length")
        .map(|v| v.to_str().ok().and_then(|s| s.parse::<i64>().ok()));
    let size_matches = match decoded_length {
        Some(n) => n == Some(blob.size),
        None => length.is_none_or(|n| n == blob.size),
    };
    let integrity_result = (|| {
        let d = digest.lock().map_err(|_| s3_error!(InternalError))?;
        if checks(headers, &blob, &d)?
            && checks(&trailer_headers, &blob, &d)?
            && has_trailers
            && size_matches
        {
            Ok(())
        } else {
            Err(s3_error!(BadDigest))
        }
    })();
    if let Err(error) = integrity_result {
        app.storage.remove(blob.id).await.map_err(domain)?;
        return Err(error);
    }
    app.upload_bytes
        .fetch_add(blob.size as u64, Ordering::Relaxed);
    Ok(blob)
}
fn checks(headers: &HeaderMap, blob: &Blob, d: &Digests) -> S3Result<bool> {
    let matches = |name: &str, value: &str| {
        headers
            .get(name)
            .is_none_or(|h| h.as_bytes() == value.as_bytes())
    };
    for name in headers.keys() {
        if name.as_str().starts_with("x-amz-checksum-")
            && !matches!(
                name.as_str(),
                "x-amz-checksum-sha256"
                    | "x-amz-checksum-crc32"
                    | "x-amz-checksum-crc32c"
                    | "x-amz-checksum-crc64nvme"
            )
        {
            return Err(s3_error!(NotImplemented));
        }
    }
    if let Some(h) = headers.get("x-amz-content-sha256") {
        let h = h.to_str().map_err(|_| s3_error!(InvalidArgument))?;
        if h.len() == 64 && h != blob.sha256 {
            return Ok(false);
        }
    }
    let sha = hex::decode(&blob.sha256).map_err(|_| s3_error!(InternalError))?;
    let md5 = hex::decode(&blob.etag).map_err(|_| s3_error!(InternalError))?;
    Ok(matches("content-md5", &STANDARD.encode(md5))
        && matches("x-amz-checksum-sha256", &STANDARD.encode(sha))
        && matches(
            "x-amz-checksum-crc32",
            &STANDARD.encode(d.crc32.clone().finalize().to_be_bytes()),
        )
        && matches(
            "x-amz-checksum-crc32c",
            &STANDARD.encode(d.crc32c.clone().finalize().to_be_bytes()),
        )
        && matches(
            "x-amz-checksum-crc64nvme",
            &STANDARD.encode(d.crc64.clone().finalize().to_be_bytes()),
        ))
}
