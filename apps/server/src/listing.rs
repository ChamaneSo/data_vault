use crate::{
    App,
    catalog::{self, Stored},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use datavault_core::Permission;
use s3s::{S3Request, S3Response, S3Result, dto::*, s3_error};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Cursor {
    bucket: Uuid,
    prefix: String,
    delimiter: String,
    after: String,
    skip_group: bool,
}
fn url_encode(s: &str) -> String {
    let mut output = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            output.push(char::from(b));
        } else {
            let _ = write!(output, "%{b:02X}");
        }
    }
    output
}
/// First string strictly beyond every UTF-8 key with this prefix, under C collation.
fn successor(prefix: &str) -> Option<String> {
    for (i, c) in prefix.char_indices().rev() {
        let next = match c as u32 {
            0x10ffff => continue,
            0xd7ff => 0xe000,
            n => n + 1,
        };
        let mut result = prefix[..i].to_owned();
        result.push(char::from_u32(next)?);
        return Some(result);
    }
    None
}

pub async fn list(
    app: &App,
    req: S3Request<ListObjectsV2Input>,
) -> S3Result<S3Response<ListObjectsV2Output>> {
    let (mut tx, bucket, _) =
        catalog::scope(app, &req, &req.input.bucket, Permission::Read).await?;
    let input = req.input;
    let max = input.max_keys.unwrap_or(1000);
    if max < 0 {
        return Err(s3_error!(InvalidArgument));
    }
    let max = max.min(1000) as usize;
    let prefix = input.prefix.clone().unwrap_or_default();
    let delimiter = input.delimiter.clone().unwrap_or_default();
    let mut after = input.start_after.clone().unwrap_or_default();
    let mut skip_group = false;
    if let Some(token) = &input.continuation_token {
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| s3_error!(InvalidArgument))?;
        let c: Cursor = serde_json::from_slice(&bytes).map_err(|_| s3_error!(InvalidArgument))?;
        if c.bucket != bucket || c.prefix != prefix || c.delimiter != delimiter {
            return Err(s3_error!(InvalidArgument));
        }
        after = c.after;
        skip_group = c.skip_group;
    }
    let escaped = |key: &str| {
        if input.encoding_type.is_some() {
            url_encode(key)
        } else {
            key.to_owned()
        }
    };
    let pattern = format!(
        "{}%",
        prefix
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_")
    );
    let mut objects = Vec::new();
    let mut groups = Vec::new();
    let mut truncated = false;
    let mut last = (after.clone(), skip_group);
    'pages: while max > 0 {
        let lower = if skip_group {
            let Some(next) = successor(&after) else { break };
            next
        } else {
            after.clone()
        };
        let rows: Vec<Stored> = sqlx::query_as(
            "SELECT * FROM objects WHERE bucket_id=$1 AND key >= $2
            AND key >= $3 AND ($4 OR key > $3) AND key LIKE $5 ESCAPE '!' ORDER BY key LIMIT 128",
        )
        .bind(bucket)
        .bind(&prefix)
        .bind(&lower)
        .bind(skip_group)
        .bind(&pattern)
        .fetch_all(&mut *tx)
        .await
        .map_err(catalog::db_error)?;
        if rows.is_empty() {
            break;
        }
        let done = rows.len() < 128;
        for row in rows {
            let common = if delimiter.is_empty() {
                None
            } else {
                row.key[prefix.len()..]
                    .find(&delimiter)
                    .map(|i| row.key[..prefix.len() + i + delimiter.len()].to_owned())
            };
            if let Some(group) = &common {
                if group <= &after {
                    continue;
                }
            }
            if objects.len() + groups.len() == max {
                truncated = true;
                break 'pages;
            }
            if let Some(group) = common {
                groups.push(CommonPrefix {
                    prefix: Some(escaped(&group)),
                    ..Default::default()
                });
                after = group;
                skip_group = true;
                last = (after.clone(), true);
                continue 'pages;
            }
            after = row.key.clone();
            skip_group = false;
            last = (after.clone(), false);
            objects.push(Object {
                last_modified: Some(row.timestamp()),
                key: Some(escaped(&row.key)),
                e_tag: Some(ETag::Strong(row.etag)),
                size: Some(row.size),
                ..Default::default()
            });
        }
        if done {
            break;
        }
        // start-after can fall inside a group which must be suppressed.
        if let Some(i) = after.strip_prefix(&prefix).and_then(|s| {
            if delimiter.is_empty() {
                None
            } else {
                s.find(&delimiter)
            }
        }) {
            after.truncate(prefix.len() + i + delimiter.len());
            skip_group = true;
        }
    }
    let next = if truncated {
        let c = Cursor {
            bucket,
            prefix: prefix.clone(),
            delimiter: delimiter.clone(),
            after: last.0,
            skip_group: last.1,
        };
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&c).map_err(|_| s3_error!(InternalError))?))
    } else {
        None
    };
    let key_count = (objects.len() + groups.len()) as i32;
    let result = ListObjectsV2Output {
        name: Some(input.bucket),
        prefix: Some(escaped(&prefix)),
        delimiter: input.delimiter.as_deref().map(escaped),
        start_after: input.start_after.as_deref().map(escaped),
        encoding_type: input.encoding_type,
        max_keys: Some(max as i32),
        key_count: Some(key_count),
        contents: Some(objects),
        common_prefixes: Some(groups),
        is_truncated: Some(truncated),
        continuation_token: input.continuation_token,
        next_continuation_token: next,
        ..Default::default()
    };
    Ok(S3Response::new(result))
}
