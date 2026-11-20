use crate::{App, catalog::db_error};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::{Aead, Payload}};
use rand::{rngs::OsRng, RngCore};
use s3s::{S3Result, auth::{S3Auth, SecretKey}, s3_error};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn random<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
pub fn matches_token(expected: &str, supplied: &str) -> bool {
    bool::from(Sha256::digest(expected.as_bytes()).ct_eq(&Sha256::digest(supplied.as_bytes())))
}

pub struct Vault(Aes256Gcm);
impl Vault {
    pub fn new(key: &[u8;32]) -> Self { Self(Aes256Gcm::new(key.into())) }
    pub fn seal(&self, id: &str, secret: &str) -> Option<Vec<u8>> {
        let mut nonce = [0u8;12];
        OsRng.fill_bytes(&mut nonce);
        let cipher = self.0.encrypt(Nonce::from_slice(&nonce), Payload { msg: secret.as_bytes(), aad: id.as_bytes() }).ok()?;
        Some([nonce.as_slice(), cipher.as_slice()].concat())
    }
    fn open(&self, id: &str, value: &[u8]) -> Option<String> {
        if value.len() < 28 { return None; }
        let plain = self.0.decrypt(Nonce::from_slice(&value[..12]),Payload { msg: &value[12..], aad:id.as_bytes() }).ok()?;
        String::from_utf8(plain).ok()
    }
}

#[async_trait::async_trait]
impl S3Auth for App {
    async fn get_secret_key(&self, access_key: &str) -> S3Result<SecretKey> {
        let cipher: Vec<u8> = sqlx::query_scalar("SELECT encrypted_secret FROM access_keys k JOIN service_accounts a ON a.id=k.account_id WHERE access_key=$1 AND revoked_at IS NULL AND enabled")
            .bind(access_key).fetch_optional(&self.db).await.map_err(db_error)?.ok_or_else(|| s3_error!(InvalidAccessKeyId))?;
        self.vault.open(access_key, &cipher).map(SecretKey::from).ok_or_else(|| s3_error!(InternalError))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cipher_is_bound_to_the_access_key() {
        let vault = Vault::new(&[42;32]);
        let bytes = vault.seal("a","test").unwrap();
        assert_eq!(vault.open("a",&bytes).as_deref(), Some("test"));
        assert!(vault.open("b",&bytes).is_none());
        assert!(!matches_token("one","two"));
    }
}
