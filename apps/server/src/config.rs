use anyhow::{Context, Result, ensure};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{collections::HashMap, env, net::SocketAddr, path::PathBuf, sync::OnceLock};
use sqlx::postgres::{PgConnectOptions, PgSslMode};

static FILE_SETTINGS: OnceLock<HashMap<String,String>> = OnceLock::new();

fn configured(name:&str)->Option<String> {
    env::var(name).ok().or_else(||FILE_SETTINGS.get()?.get(name).cloned())
}
pub fn setting(name: &str, default: &str) -> String {
    configured(&format!("DATAVAULT_{name}")).unwrap_or_else(|| default.into())
}
pub struct Config {
    pub db: PgConnectOptions,
    pub root: PathBuf,
    pub s3: SocketAddr,
    pub admin: SocketAddr,
    pub admin_token: String,
    pub key: [u8; 32],
    pub tls: Option<(PathBuf, PathBuf)>,
    pub region: String,
    pub metrics: bool,
}
fn decode_master_key(encoded: &str) -> Result<[u8; 32]> {
    STANDARD.decode(encoded).ok().and_then(|v| v.try_into().ok())
        .context("Master key must encode 32 random bytes as base64")
}
impl Config {
    pub fn load() -> Result<Self> {
        if let Some(path)=env::var_os("DATAVAULT_CONFIG") {
            let pairs=dotenvy::from_path_iter(PathBuf::from(path)).map_err(|_| anyhow::anyhow!("Cannot read DATAVAULT_CONFIG"))?
                .collect::<std::result::Result<HashMap<_,_>,_>>().map_err(|_| anyhow::anyhow!("Invalid configuration file"))?;
            let _=FILE_SETTINGS.set(pairs);
        }
        let admin_token = configured("DATAVAULT_ADMIN_TOKEN").context("Set DATAVAULT_ADMIN_TOKEN")?;
        ensure!(admin_token.len() >= 32, "Admin token needs at least 32 bytes");
        let key = decode_master_key(&configured("DATAVAULT_MASTER_KEY").context("Set DATAVAULT_MASTER_KEY")?)?;
        let dev = match setting("ENV","production").as_str() {
            "development" => true, "production" => false, _ => anyhow::bail!("Invalid DATAVAULT_ENV"),
        };
        let tls = match (configured("DATAVAULT_TLS_CERT"), configured("DATAVAULT_TLS_KEY")) {
            (Some(cert), Some(key)) => Some((cert.into(), key.into())),
            (None,None) if dev => None,
            _ => anyhow::bail!("Configure both DATAVAULT_TLS_CERT and DATAVAULT_TLS_KEY"),
        };
        let address = |kind: &str, port: &str| -> Result<SocketAddr> {
            Ok(SocketAddr::new(setting(&format!("{kind}_HOST"), "127.0.0.1").parse()?,
                setting(&format!("{kind}_PORT"), port).parse()?))
        };
        let password = configured("DATAVAULT_DATABASE_PASSWORD").context("Set database password")?;
        ensure!(!password.is_empty(),"Database password must contain at least one character");
        let mut db = PgConnectOptions::new().host(&setting("DATABASE_HOST","localhost"))
            .port(setting("DATABASE_PORT","5432").parse()?).database(&setting("DATABASE_NAME","datavault"))
            .username(&setting("DATABASE_USER","datavault")).password(&password)
            .ssl_mode(if dev { PgSslMode::Prefer } else { PgSslMode::VerifyFull });
        if let Some(ca) = configured("DATAVAULT_DATABASE_CA") { db = db.ssl_root_cert(PathBuf::from(ca)); }
        Ok(Self { db, root: setting("STORAGE_PATH","/var/lib/datavault").into(), s3: address("S3","9000")?,
            admin: address("ADMIN","9001")?, admin_token, key, tls, region: setting("REGION","us-east-1"),
            metrics: setting("METRICS_ENABLED","true").parse()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_requires_exactly_32_bytes() {
        assert_eq!(decode_master_key(&STANDARD.encode([9; 32])).unwrap(), [9; 32]);
        assert!(decode_master_key(&STANDARD.encode([9; 31])).is_err());
        assert!(decode_master_key("invalid base64").is_err());
    }
}
