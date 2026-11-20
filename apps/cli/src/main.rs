//! Administration over HTTP and storage operations through the bundled AWS CLI.
use anyhow::{Context,Result,bail,ensure};
use clap::{Parser,Subcommand};
use reqwest::{Client,Method};
use serde_json::{Value,json};
use std::{env,process::Stdio};
use tokio::process::Command;

#[derive(Parser)]
#[command(version,about="Manage a DataVault storage server")]
struct Cli {
    #[arg(long,env="DATAVAULT_ADMIN_ENDPOINT",default_value="http://127.0.0.1:9001",global=true)]
    admin_endpoint:String,
    #[arg(long,env="DATAVAULT_S3_ENDPOINT",default_value="http://127.0.0.1:9000",global=true)]
    s3_endpoint:String,
    #[command(subcommand)] cmd:Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    #[command(subcommand)] Server(Server),
    #[command(subcommand)] User(User),
    #[command(subcommand)] AccessKey(AccessKey),
    #[command(subcommand)] Permission(Permission),
    #[command(subcommand)] Storage(Storage),
    #[command(subcommand)] Bucket(Bucket),
    #[command(subcommand)] Object(Object),
}
#[derive(Subcommand)]enum Server{Status}
#[derive(Subcommand)]enum Storage{Stats}
#[derive(Subcommand)]enum User{Create{name:String,#[arg(long)]can_create_buckets:bool},List,Disable{id:String}}
#[derive(Subcommand)]enum AccessKey{Create{user:String},List{user:String},Revoke{id:String}}
#[derive(Subcommand)]enum Permission{Grant{user:String,role:String,bucket:String},List,Revoke{id:String}}
#[derive(Subcommand)]enum Bucket{Create{name:String},List,Delete{name:String}}
#[derive(Subcommand)]enum Object{Upload{file:String,bucket:String,key:String},Download{bucket:String,key:String,destination:String},List{bucket:String},Delete{bucket:String,key:String}}

async fn aws(endpoint:&str,args:&[&str])->Result<()> {
    let status=Command::new("aws").args(["--endpoint-url",endpoint]).args(args)
        .env("AWS_PAGER","").stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit())
        .status().await.context("Start the AWS CLI transport included in the DataVault image")?;
    ensure!(status.success(),"S3 request failed");
    Ok(())
}
#[tokio::main]
async fn main()->Result<()> {
    let cli=Cli::parse();
    let s3=&cli.s3_endpoint;
    let (method,path,body)=match cli.cmd {
        Cmd::Server(Server::Status)=>(Method::GET,"server/info".into(),None),
        Cmd::Storage(Storage::Stats)=>(Method::GET,"storage/stats".into(),None),
        Cmd::User(User::Create{name,can_create_buckets})=>(Method::POST,"service-accounts".into(),Some(json!({"name":name,"can_create_buckets":can_create_buckets}))),
        Cmd::User(User::List)=>(Method::GET,"service-accounts".into(),None),
        Cmd::User(User::Disable{id})=>(Method::DELETE,format!("service-accounts/{id}"),None),
        Cmd::AccessKey(AccessKey::Create{user})=>(Method::POST,format!("service-accounts/{user}/access-keys"),None),
        Cmd::AccessKey(AccessKey::List{user})=>(Method::GET,format!("service-accounts/{user}/access-keys"),None),
        Cmd::AccessKey(AccessKey::Revoke{id})=>(Method::DELETE,format!("access-keys/{id}"),None),
        Cmd::Permission(Permission::Grant{user,role,bucket})=>(Method::POST,"permissions".into(),Some(json!({"account_id":user,"role":role,"bucket":bucket}))),
        Cmd::Permission(Permission::List)=>(Method::GET,"permissions".into(),None),
        Cmd::Permission(Permission::Revoke{id})=>(Method::DELETE,format!("permissions/{id}"),None),
        Cmd::Bucket(Bucket::List)=>return aws(s3,&["s3api","list-buckets"]).await,
        Cmd::Bucket(Bucket::Create{name})=>return aws(s3,&["s3api","create-bucket","--bucket",&name]).await,
        Cmd::Bucket(Bucket::Delete{name})=>return aws(s3,&["s3api","delete-bucket","--bucket",&name]).await,
        Cmd::Object(Object::List{bucket})=>return aws(s3,&["s3api","list-objects-v2","--bucket",&bucket]).await,
        Cmd::Object(Object::Delete{bucket,key})=>return aws(s3,&["s3api","delete-object","--bucket",&bucket,"--key",&key]).await,
        Cmd::Object(Object::Upload{file,bucket,key})=>return aws(s3,&["s3","cp",&file,&format!("s3://{bucket}/{key}")]).await,
        Cmd::Object(Object::Download{bucket,key,destination})=>return aws(s3,&["s3","cp",&format!("s3://{bucket}/{key}"),&destination]).await,
    };
    let token=env::var("DATAVAULT_ADMIN_TOKEN").context("Set DATAVAULT_ADMIN_TOKEN")?;
    let client=Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let mut request=client.request(method,format!("{}/api/v1/{path}",cli.admin_endpoint.trim_end_matches('/'))).bearer_auth(token);
    if let Some(body)=body {request=request.json(&body);}
    let res=request.send().await?;
    if !res.status().is_success() {bail!("Admin request failed with {}",res.status());}
    println!("{}",serde_json::to_string_pretty(&res.json::<Value>().await?)?);
    Ok(())
}
