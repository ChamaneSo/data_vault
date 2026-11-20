mod admin;
mod body;
mod catalog;
mod config;
mod filesystem;
mod listing;
mod multipart;
mod s3;
mod security;

use anyhow::{Context, Result, ensure};
use axum::{Router, body::Body, extract::{ConnectInfo,Request,State}, http::{HeaderValue,StatusCode}, middleware::{self,Next}, response::{IntoResponse,Response}};
use axum_server::tls_rustls::RustlsConfig;
use datavault_core::Storage;
use s3s::{config::{S3Config,StaticConfigProvider},service::S3ServiceBuilder};
use sqlx::{Connection,PgConnection,PgPool,postgres::PgPoolOptions};
use std::{collections::HashSet,net::SocketAddr,ops::Deref,sync::{Arc,OnceLock,atomic::{AtomicU64,Ordering}},time::{Duration,Instant}};
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Clone)]
struct App(Arc<Inner>);
struct Inner {
    db:PgPool, storage:Arc<dyn Storage>, vault:security::Vault, token:String, metrics:bool,
    requests:AtomicU64, errors:AtomicU64, micros:AtomicU64, upload_bytes:AtomicU64, download_bytes:AtomicU64,
}
impl Deref for App {type Target=Inner;fn deref(&self)->&Inner{&self.0}}

#[derive(Clone,Default)]
struct RequestContext {
    principal:Arc<OnceLock<Uuid>>,
}
impl RequestContext {
    fn set_principal(&self,id:Uuid){ let _=self.principal.set(id); }
}
async fn observe(State((app, interface)):State<(App, &'static str)>,mut req:Request,next:Next)->Response {
    let id=Uuid::new_v4();
    let ctx=RequestContext::default();
    let ip=req.extensions().get::<ConnectInfo<SocketAddr>>().map(|s|s.0.ip().to_string());
    let method=req.method().clone();
    let path=req.uri().path().to_owned();
    let start=Instant::now();
    req.extensions_mut().insert(ctx.clone());
    let mut response=next.run(req).await;
    let status=response.status();
    if let Ok(value)=HeaderValue::from_str(&id.to_string()) {response.headers_mut().insert("x-amz-request-id",value);}
    let micros=start.elapsed().as_micros() as u64;
    app.requests.fetch_add(1,Ordering::Relaxed);
    app.micros.fetch_add(micros,Ordering::Relaxed);
    if status.is_server_error() || status.is_client_error() {app.errors.fetch_add(1,Ordering::Relaxed);}
    tracing::info!(request_id=%id,%method,status=status.as_u16(),duration_us=micros,"HTTP request");
    if status.is_client_error() || status.is_server_error() || matches!(method.as_str(),"POST"|"PUT"|"DELETE") {
        let action=format!("{interface}.{method}");
        if sqlx::query("INSERT INTO audit_events(request_id,principal_id,action,resource,result,source_ip) VALUES($1,$2,$3,$4,$5,$6::inet)")
            .bind(id).bind(ctx.principal.get().copied()).bind(action).bind(path).bind(status.as_u16().to_string()).bind(ip)
            .execute(&app.db).await.is_err() {tracing::error!(request_id=%id,"audit write failed");}
    }
    response
}
async fn metrics(State(app):State<App>)->Response {
    let Ok((buckets,objects,bytes))=admin::counts(&app).await else {return StatusCode::SERVICE_UNAVAILABLE.into_response()};
    let load=|value:&AtomicU64|value.load(Ordering::Relaxed);
    let data=format!(concat!(
        "# TYPE datavault_http_requests_total counter\ndatavault_http_requests_total {}\n",
        "# TYPE datavault_http_request_duration_seconds summary\ndatavault_http_request_duration_seconds_sum {}\ndatavault_http_request_duration_seconds_count {}\n",
        "datavault_errors_total {}\ndatavault_storage_upload_bytes_total {}\ndatavault_storage_download_bytes_total {}\n",
        "datavault_storage_buckets_total {}\ndatavault_storage_objects_total {}\ndatavault_storage_bytes_used {}\n"),
        load(&app.requests), load(&app.micros) as f64 / 1e6,load(&app.requests),load(&app.errors),
        load(&app.upload_bytes),load(&app.download_bytes),buckets,objects,bytes);
    ([("content-type","text/plain; version=0.0.4")],data).into_response()
}

async fn serve(addr:SocketAddr,router:Router,tls:Option<RustlsConfig>)->Result<()> {
    let make=router.into_make_service_with_connect_info::<SocketAddr>();
    if let Some(config)=tls {
        axum_server::bind_rustls(addr,config).serve(make).await?;
    } else {
        let listener=tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener,make).await?;
    }
    Ok(())
}
async fn monitor(mut lease:PgConnection)->Result<()> {
    loop { tokio::time::sleep(Duration::from_secs(2)).await;lease.ping().await.context("Lost catalog ownership connection")?; }
}

#[tokio::main]
async fn main()->Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let archive = match args.as_slice() {
        [] => false,
        [command] if command == "archive-unused" => true,
        _ => anyhow::bail!("Usage: datavault-server [archive-unused]"),
    };
    let cfg=config::Config::load()?;
    let level: tracing::Level = config::setting("LOG_LEVEL","info").parse()?;
    let filter = EnvFilter::try_new(format!("warn,datavault_server={level},s3s=off"))?;
    tracing_subscriber::fmt().json().with_env_filter(filter).init();
    let _=rustls::crypto::ring::default_provider().install_default();
    let mut lease=PgConnection::connect_with(&cfg.db).await.context("Open catalog ownership connection")?;
    let exclusive:bool=sqlx::query_scalar("SELECT pg_try_advisory_lock(3499207001464::bigint)").fetch_one(&mut lease).await?;
    ensure!(exclusive,"A DataVault process already owns this database");
    let db=PgPoolOptions::new().max_connections(24).acquire_timeout(Duration::from_secs(10)).connect_with(cfg.db).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    let fs=filesystem::Filesystem::open(&cfg.root).await?;
    let catalog:Uuid=sqlx::query_scalar("SELECT id FROM catalog_identity").fetch_one(&db).await?;
    let refs:HashSet<Uuid>=sqlx::query_scalar("SELECT storage_id FROM objects UNION SELECT storage_id FROM parts")
        .fetch_all(&db).await?.into_iter().collect();
    fs.bind_catalog(catalog,!refs.is_empty()).await?;
    if archive {
        let batch = fs.archive_unused(&refs).await?;
        tracing::info!(directory=%batch.display(),"offline recovery archive complete");
        return Ok(());
    }
    let (unreferenced,temporary)=fs.inspect(&refs).await?;
    tracing::info!(unreferenced,temporary,"storage reconciliation completed");
    let app=App(Arc::new(Inner {db,storage:Arc::new(fs),vault:security::Vault::new(&cfg.key),token:cfg.admin_token,
        metrics:cfg.metrics,requests:0.into(),errors:0.into(),micros:0.into(),upload_bytes:0.into(),download_bytes:0.into()}));
    let mut builder=S3ServiceBuilder::new(app.clone());
    builder.set_auth(app.clone());
    let mut protocol=S3Config::default();
    protocol.expected_region=Some(cfg.region.parse()?);
    protocol.sig_v4_allowed_services=vec!["s3".into()];
    protocol.enable_sig_v2=false;
    protocol.allow_presigned_url=true;
    protocol.allow_post_signature=false;
    protocol.normalize_forward_slash_path=false;
    protocol.presigned_url_max_skew_time_secs=900;
    builder.set_config(Arc::new(StaticConfigProvider::new(Arc::new(protocol))));
    let svc=builder.build();
    let s3=Router::new().fallback(move |req:Request| {
        let svc=svc.clone();
        async move {match svc.oneshot(req).await {
            Ok(resp)=>resp.map(Body::new),
            Err(_)=>(StatusCode::INTERNAL_SERVER_ERROR,[("content-type","application/xml")],"<Error><Code>InternalError</Code></Error>").into_response(),
        }}
    });
    let s3=s3.layer(middleware::from_fn_with_state((app.clone(),"s3"),observe));
    let admin=admin::router(app.clone()).layer(middleware::from_fn_with_state((app,"admin"),observe));
    let tls=match cfg.tls {Some((cert,key))=>Some(RustlsConfig::from_pem_file(cert,key).await?),None=>None};
    tracing::info!(s3=%cfg.s3,admin=%cfg.admin,tls=tls.is_some(),"starting DataVault");
    tokio::try_join!(serve(cfg.s3,s3,tls.clone()),serve(cfg.admin,admin,tls),monitor(lease))?;
    Ok(())
}
