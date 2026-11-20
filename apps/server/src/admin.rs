use crate::{App, security::{self, matches_token}};
use axum::{Json, Router, extract::{Path, State, Request, DefaultBodyLimit}, http::StatusCode,
    middleware::{self, Next}, response::{IntoResponse,Response}, routing::{get,post,delete}};
use datavault_core::Role;
use serde::Deserialize;
use serde_json::{Value,json};
use sqlx::types::Json as DbJson;
use uuid::Uuid;

#[derive(Debug)]
struct ApiError(StatusCode,&'static str);
impl IntoResponse for ApiError {
    fn into_response(self)->Response { (self.0,Json(json!({"error":self.1}))).into_response() }
}
impl From<sqlx::Error> for ApiError {
    fn from(e:sqlx::Error)->Self {
        if let Some(db)=e.as_database_error() {
            if db.is_unique_violation(){return Self(StatusCode::CONFLICT,"Resource already exists");}
            if db.is_foreign_key_violation(){return Self(StatusCode::BAD_REQUEST,"Invalid resource reference");}
        }
        tracing::error!("admin database operation failed");
        Self(StatusCode::SERVICE_UNAVAILABLE,"Metadata unavailable")
    }
}
type ApiResult=Result<Json<Value>,ApiError>;
fn missing()->ApiError {ApiError(StatusCode::NOT_FOUND,"Resource not found")}
fn invalid()->ApiError {ApiError(StatusCode::BAD_REQUEST,"Invalid request")}
fn id(s:&str)->Result<Uuid,ApiError>{Uuid::parse_str(s).map_err(|_|invalid())}

async fn guard(State(app):State<App>,req:Request,next:Next)->Response {
    let token=req.headers().get("authorization").and_then(|h|h.to_str().ok()).and_then(|v|v.strip_prefix("Bearer "));
    if !token.is_some_and(|t|matches_token(&app.token,t)) {
        return ApiError(StatusCode::UNAUTHORIZED,"Administrator authentication required").into_response();
    }
    next.run(req).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAccount {name:String,#[serde(default)] can_create_buckets:bool}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {account_id:Uuid,bucket:String,role:Role}

async fn create(State(a):State<App>,body:Result<Json<CreateAccount>,axum::extract::rejection::JsonRejection>)->ApiResult {
    let Json(body)=body.map_err(|_|invalid())?;
    if body.name.is_empty() || body.name.len()>128 || !body.name.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_.".contains(&b)) {return Err(invalid());}
    let id=Uuid::new_v4();
    sqlx::query("INSERT INTO service_accounts(id,name,can_create_buckets) VALUES($1,$2,$3)").bind(id).bind(&body.name).bind(body.can_create_buckets).execute(&a.db).await?;
    Ok(Json(json!({"id":id,"name":body.name,"can_create_buckets":body.can_create_buckets})))
}
async fn accounts(State(a):State<App>)->ApiResult {
    let rows:Vec<DbJson<Value>>=sqlx::query_scalar("SELECT to_jsonb(s) FROM service_accounts s ORDER BY name").fetch_all(&a.db).await?;
    Ok(Json(json!(rows)))
}
async fn account(State(a):State<App>,Path(uid):Path<String>)->ApiResult {
    let row:DbJson<Value>=sqlx::query_scalar("SELECT to_jsonb(s) FROM service_accounts s WHERE id=$1").bind(id(&uid)?).fetch_optional(&a.db).await?.ok_or_else(missing)?;
    Ok(Json(row.0))
}
async fn disable(State(a):State<App>,Path(uid):Path<String>)->ApiResult {
    let r=sqlx::query("UPDATE service_accounts SET enabled=false WHERE id=$1").bind(id(&uid)?).execute(&a.db).await?;
    if r.rows_affected()==0 {return Err(missing());}
    Ok(Json(json!({"enabled":false})))
}
async fn new_key(State(a):State<App>,Path(uid):Path<String>)->ApiResult {
    let uid=id(&uid)?;
    let id=Uuid::new_v4();
    let access=format!("DV{}",security::random::<10>().to_ascii_uppercase());
    let secret=security::random::<32>();
    let cipher=a.vault.seal(&access,&secret).ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR,"Credential generation failed"))?;
    let r=sqlx::query("INSERT INTO access_keys(id,account_id,access_key,encrypted_secret) SELECT $1,id,$3,$4 FROM service_accounts WHERE id=$2 AND enabled")
        .bind(id).bind(uid).bind(&access).bind(cipher).execute(&a.db).await?;
    if r.rows_affected()==0 {return Err(missing());}
    Ok(Json(json!({"id":id,"access_key":access,"secret_key":secret})))
}
async fn keys(State(a):State<App>,Path(uid):Path<String>)->ApiResult {
    let rows:Vec<DbJson<Value>>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'access_key',access_key,'created_at',created_at,'revoked_at',revoked_at) FROM access_keys WHERE account_id=$1 ORDER BY created_at")
        .bind(id(&uid)?).fetch_all(&a.db).await?;
    Ok(Json(json!(rows)))
}
async fn revoke(State(a):State<App>,Path(key):Path<String>)->ApiResult {
    let r=sqlx::query("UPDATE access_keys SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1").bind(id(&key)?).execute(&a.db).await?;
    if r.rows_affected()==0{return Err(missing());}
    Ok(Json(json!({"revoked":true})))
}
async fn grant(State(a):State<App>,body:Result<Json<Grant>,axum::extract::rejection::JsonRejection>)->ApiResult {
    let Json(b)=body.map_err(|_|invalid())?;
    let id:Uuid=sqlx::query_scalar("INSERT INTO policy_bindings(id,account_id,bucket_id,role)
        SELECT $1,s.id,b.id,$4 FROM service_accounts s,buckets b WHERE s.id=$2 AND s.enabled AND b.name=$3
        ON CONFLICT(account_id,bucket_id) DO UPDATE SET role=excluded.role RETURNING id")
        .bind(Uuid::new_v4()).bind(b.account_id).bind(b.bucket).bind(b.role.as_str()).fetch_optional(&a.db).await?.ok_or_else(missing)?;
    Ok(Json(json!({"id":id})))
}
async fn permissions(State(a):State<App>)->ApiResult {
    let rows:Vec<DbJson<Value>>=sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('bucket',b.name) FROM policy_bindings p JOIN buckets b ON b.id=p.bucket_id ORDER BY b.name,p.account_id").fetch_all(&a.db).await?;
    Ok(Json(json!(rows)))
}
async fn ungrant(State(a):State<App>,Path(binding):Path<String>)->ApiResult {
    let r=sqlx::query("DELETE FROM policy_bindings WHERE id=$1").bind(id(&binding)?).execute(&a.db).await?;
    if r.rows_affected()==0{return Err(missing());}
    Ok(Json(json!({"removed":true})))
}
pub async fn counts(a:&App)->Result<(i64,i64,i64),sqlx::Error>{
    sqlx::query_as("SELECT (SELECT count(*) FROM buckets),count(*),coalesce(sum(size),0)::bigint FROM objects").fetch_one(&a.db).await
}
async fn stats(State(a):State<App>)->ApiResult {
    let (buckets,objects,bytes)=counts(&a).await?;
    Ok(Json(json!({"buckets":buckets,"objects":objects,"logical_bytes":bytes})))
}
async fn info()->Json<Value>{Json(json!({"name":"DataVault","version":env!("CARGO_PKG_VERSION"),"mode":"single-node"}))}
async fn ready(State(a):State<App>)->Response {
    if sqlx::query("SELECT 1").execute(&a.db).await.is_err() || a.storage.ready().await.is_err() {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE,"Dependency unavailable").into_response();
    }
    Json(json!({"ready":true})).into_response()
}
pub fn router(a:App)->Router {
    let protected=Router::new().route("/service-accounts",get(accounts).post(create))
        .route("/service-accounts/{id}",get(account).delete(disable))
        .route("/service-accounts/{id}/access-keys",post(new_key).get(keys))
        .route("/access-keys/{id}",delete(revoke)).route("/permissions",post(grant).get(permissions))
        .route("/permissions/{id}",delete(ungrant)).route("/storage/stats",get(stats)).route("/server/info",get(info))
        .route_layer(middleware::from_fn_with_state(a.clone(),guard));
    let mut r=Router::new().nest("/api/v1",protected)
        .route("/health/live",get(||async {Json(json!({"live":true}))})).route("/health/ready",get(ready));
    if a.metrics {r=r.route("/metrics",get(crate::metrics));}
    r.fallback(|| async { ApiError(StatusCode::NOT_FOUND, "Endpoint not found") })
        .method_not_allowed_fallback(|| async { ApiError(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed") })
        .layer(DefaultBodyLimit::max(16*1024)).with_state(a)
}
