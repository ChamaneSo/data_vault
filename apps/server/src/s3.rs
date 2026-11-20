use crate::{App, body, catalog::{self, db_error, domain}, listing, multipart};
use datavault_core::{Permission, bucket_name, object_key};
use s3s::{S3, S3Request, S3Response, S3Result, dto::*, s3_error};
use chrono::{DateTime,Utc};
use std::{sync::atomic::Ordering,time::SystemTime};
use uuid::Uuid;

#[async_trait::async_trait]
impl S3 for App {
    async fn list_buckets(&self, req: S3Request<ListBucketsInput>) -> S3Result<S3Response<ListBucketsOutput>> {
        let (mut tx,account,_) = catalog::identity(self,&req).await?;
        let input=req.input;
        let max=input.max_buckets.unwrap_or(1000).clamp(1,10000);
        let rows: Vec<(String,DateTime<Utc>)> = sqlx::query_as("SELECT name,created_at FROM buckets b WHERE
            (owner_id=$1 OR EXISTS(SELECT 1 FROM policy_bindings p WHERE p.bucket_id=b.id AND p.account_id=$1))
            AND name > $2 AND starts_with(name,$3) ORDER BY name LIMIT $4")
            .bind(account).bind(input.continuation_token.as_deref().unwrap_or("")).bind(input.prefix.as_deref().unwrap_or(""))
            .bind(i64::from(max)+1).fetch_all(&mut *tx).await.map_err(db_error)?;
        let token=if rows.len()>max as usize { Some(rows[max as usize-1].0.clone()) } else { None };
        let buckets=rows.into_iter().take(max as usize).map(|(name,created)| Bucket {
            name:Some(name),creation_date:Some(Timestamp::from(SystemTime::from(created))),..Default::default()
        }).collect();
        Ok(S3Response::new(ListBucketsOutput { buckets:Some(buckets),continuation_token:token,..Default::default() }))
    }
    async fn create_bucket(&self, req: S3Request<CreateBucketInput>) -> S3Result<S3Response<CreateBucketOutput>> {
        bucket_name(&req.input.bucket).map_err(|_| s3_error!(InvalidBucketName))?;
        let (mut tx,account,allowed) = catalog::identity(self,&req).await?;
        if !allowed { return Err(s3_error!(AccessDenied)); }
        if req.input.acl.is_some() { return Err(s3_error!(NotImplemented)); }
        let name=req.input.bucket;
        let inserted=sqlx::query("INSERT INTO buckets(id,name,owner_id) VALUES($1,$2,$3) ON CONFLICT(name) DO NOTHING")
            .bind(Uuid::new_v4()).bind(&name).bind(account).execute(&mut *tx).await.map_err(db_error)?;
        if inserted.rows_affected()==0 { return Err(s3_error!(BucketAlreadyExists)); }
        tx.commit().await.map_err(db_error)?;
        Ok(S3Response::new(CreateBucketOutput { location:Some(format!("/{name}")),..Default::default() }))
    }
    async fn head_bucket(&self, req: S3Request<HeadBucketInput>) -> S3Result<S3Response<HeadBucketOutput>> {
        catalog::scope(self,&req,&req.input.bucket,Permission::Read).await?;
        Ok(S3Response::new(HeadBucketOutput::default()))
    }
    async fn delete_bucket(&self, req: S3Request<DeleteBucketInput>) -> S3Result<S3Response<DeleteBucketOutput>> {
        let (mut tx,id,_)=catalog::scope(self,&req,&req.input.bucket,Permission::Manage).await?;
        let r=sqlx::query("DELETE FROM buckets WHERE id=$1").bind(id).execute(&mut *tx).await;
        match r {
            Err(e) if e.as_database_error().is_some_and(|e| e.is_foreign_key_violation()) => return Err(s3_error!(BucketNotEmpty)),
            other => { other.map_err(db_error)?; }
        }
        tx.commit().await.map_err(db_error)?;
        Ok(S3Response::new(DeleteBucketOutput::default()))
    }
    async fn put_object(&self, req: S3Request<PutObjectInput>) -> S3Result<S3Response<PutObjectOutput>> {
        let (mut tx,bucket,_) = catalog::scope(self,&req,&req.input.bucket,Permission::Write).await?;
        let input=req.input;
        object_key(&input.key).map_err(domain)?;
        if input.acl.is_some() || input.server_side_encryption.is_some() { return Err(s3_error!(NotImplemented)); }
        catalog::check_write(&mut tx,bucket,&input.key,input.if_match.as_ref(),input.if_none_match.as_ref()).await?;
        let blob=body::upload(self,&req.headers,input.body,input.content_length,||req.trailing_headers.as_ref().and_then(|t|t.take())).await?;
        catalog::publish(&mut tx,bucket,&input.key,&blob,input.content_type.as_deref().unwrap_or("application/octet-stream"),input.metadata.unwrap_or_default()).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(S3Response::new(PutObjectOutput { e_tag:Some(ETag::Strong(blob.etag)),checksum_crc32:input.checksum_crc32,checksum_sha256:input.checksum_sha256,..Default::default() }))
    }
    async fn get_object(&self, req: S3Request<GetObjectInput>) -> S3Result<S3Response<GetObjectOutput>> {
        let (mut tx,bucket,_)=catalog::scope(self,&req,&req.input.bucket,Permission::Read).await?;
        let input=req.input;
        if input.version_id.is_some() || input.part_number.is_some() { return Err(s3_error!(NotImplemented)); }
        let row=catalog::get(&mut tx,bucket,&input.key).await?;
        catalog::conditions(Some(&row.etag),input.if_match.as_ref(),input.if_none_match.as_ref(),true)?;
        let size=row.size as u64;
        if size == 0 && input.range.is_some() {
            return Err(s3_error!(InvalidRange));
        }
        let range=match &input.range { Some(r)=>r.check(size).map_err(|_| s3_error!(InvalidRange))?,None=>0..size };
        let length=range.end-range.start;
        let blob=row.blob();
        tx.commit().await.map_err(db_error)?;
        self.storage.verify(&blob).await.map_err(|_| s3_error!(InternalError))?;
        let stream=self.storage.read(&blob,range.start,length).await.map_err(domain)?;
        let partial=input.range.is_some();
        let mut out=S3Response::new(GetObjectOutput { last_modified:Some(row.timestamp()),accept_ranges:Some("bytes".into()),
            content_type:Some(row.content_type), metadata:Some(row.metadata.0), e_tag:Some(ETag::Strong(row.etag)),
            content_range:partial.then(|| format!("bytes {}-{}/{}",range.start,range.end.saturating_sub(1),size)),
            content_length:Some(length as i64),body:Some(StreamingBlob::wrap(sync_wrapper::SyncStream::new(stream))),..Default::default() });
        if partial { out.status=Some(axum::http::StatusCode::PARTIAL_CONTENT); }
        self.download_bytes.fetch_add(length,Ordering::Relaxed);
        Ok(out)
    }
    async fn head_object(&self, req: S3Request<HeadObjectInput>) -> S3Result<S3Response<HeadObjectOutput>> {
        let (mut tx,bucket,_)=catalog::scope(self,&req,&req.input.bucket,Permission::Read).await?;
        let i=req.input;
        if i.version_id.is_some() || i.range.is_some() { return Err(s3_error!(NotImplemented)); }
        let row=catalog::get(&mut tx,bucket,&i.key).await?;
        catalog::conditions(Some(&row.etag),i.if_match.as_ref(),i.if_none_match.as_ref(),true)?;
        if self.storage.size(row.storage_id).await.map_err(domain)? != Some(row.size as u64) { return Err(s3_error!(InternalError)); }
        Ok(S3Response::new(HeadObjectOutput { last_modified:Some(row.timestamp()),content_length:Some(row.size),
            content_type:Some(row.content_type),metadata:Some(row.metadata.0),e_tag:Some(ETag::Strong(row.etag)),..Default::default() }))
    }
    async fn delete_object(&self, req: S3Request<DeleteObjectInput>) -> S3Result<S3Response<DeleteObjectOutput>> {
        let (mut tx,bucket,_)=catalog::scope(self,&req,&req.input.bucket,Permission::Write).await?;
        let i=req.input;
        if i.version_id.is_some() { return Err(s3_error!(NotImplemented)); }
        catalog::check_write(&mut tx,bucket,&i.key,i.if_match.as_ref(),None).await?;
        sqlx::query("DELETE FROM objects WHERE bucket_id=$1 AND key=$2").bind(bucket).bind(i.key)
            .execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(S3Response::new(DeleteObjectOutput::default()))
    }
    async fn copy_object(&self, req: S3Request<CopyObjectInput>) -> S3Result<S3Response<CopyObjectOutput>> {
        let (mut tx,dest,account)=catalog::scope(self,&req,&req.input.bucket,Permission::Write).await?;
        let i=req.input;
        object_key(&i.key).map_err(domain)?;
        let CopySource::Bucket {bucket,key,version_id:None} = i.copy_source else {return Err(s3_error!(NotImplemented))};
        let source=catalog::authorize(&mut tx,account,&bucket,Permission::Read).await?;
        let row=catalog::get(&mut tx,source,&key).await?;
        catalog::conditions(Some(&row.etag),i.copy_source_if_match.as_ref(),i.copy_source_if_none_match.as_ref(),true)?;
        catalog::check_write(&mut tx,dest,&i.key,i.if_match.as_ref(),i.if_none_match.as_ref()).await?;
        let source_blob=row.blob();
        self.storage.verify(&source_blob).await.map_err(domain)?;
        let stream=self.storage.read(&source_blob,0,row.size as u64).await.map_err(domain)?;
        let blob=self.storage.put(stream,body::MAX_PART).await.map_err(domain)?;
        let replace=i.metadata_directive.as_ref().is_some_and(|v| v.as_str()=="REPLACE");
        let (ct,meta)=if replace {(i.content_type.unwrap_or_else(||"application/octet-stream".into()),i.metadata.unwrap_or_default())} else {(row.content_type,row.metadata.0)};
        catalog::publish(&mut tx,dest,&i.key,&blob,&ct,meta).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(S3Response::new(CopyObjectOutput {copy_object_result:Some(CopyObjectResult {e_tag:Some(ETag::Strong(blob.etag)),
            last_modified:Some(Timestamp::from(SystemTime::now())),..Default::default()}),..Default::default()}))
    }
    async fn list_objects_v2(&self, req:S3Request<ListObjectsV2Input>)->S3Result<S3Response<ListObjectsV2Output>> { listing::list(self,req).await }
    async fn create_multipart_upload(&self, req:S3Request<CreateMultipartUploadInput>)->S3Result<S3Response<CreateMultipartUploadOutput>> { multipart::create(self,req).await }
    async fn upload_part(&self, req:S3Request<UploadPartInput>)->S3Result<S3Response<UploadPartOutput>> { multipart::part(self,req).await }
    async fn list_parts(&self, req:S3Request<ListPartsInput>)->S3Result<S3Response<ListPartsOutput>> { multipart::list(self,req).await }
    async fn complete_multipart_upload(&self, req:S3Request<CompleteMultipartUploadInput>)->S3Result<S3Response<CompleteMultipartUploadOutput>> { multipart::complete(self,req).await }
    async fn abort_multipart_upload(&self, req:S3Request<AbortMultipartUploadInput>)->S3Result<S3Response<AbortMultipartUploadOutput>> { multipart::abort(self,req).await }
}
