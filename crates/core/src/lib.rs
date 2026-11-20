//! Shared storage contracts, resource validation and bucket-scoped permissions.
use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::{io, pin::Pin};
use uuid::Uuid;

pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid request")]
    Invalid,
    #[error("Access denied")]
    Denied,
    #[error("Bucket already exists")]
    BucketExists,
    #[error("Bucket does not exist")]
    NoBucket,
    #[error("Bucket is not empty")]
    BucketNotEmpty,
    #[error("Object does not exist")]
    NoObject,
    #[error("Entity too large")]
    TooLarge,
    #[error("Checksum mismatch")]
    Checksum,
    #[error("Invalid range")]
    Range,
    #[error("Precondition failed")]
    Precondition,
    #[error("Storage unavailable")]
    Storage,
    #[error("Metadata unavailable")]
    Database,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Role {
    Admin,
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy)]
pub enum Permission {
    Read,
    Write,
    Manage,
}

impl Role {
    pub fn permits(self, action: Permission) -> bool {
        match action {
            Permission::Read => true,
            Permission::Write => matches!(self, Self::Admin | Self::ReadWrite),
            Permission::Manage => self == Self::Admin,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "Admin",
            Self::ReadWrite => "ReadWrite",
            Self::ReadOnly => "ReadOnly",
        }
    }
}

pub fn bucket_name(name: &str) -> Result<()> {
    let valid = (3..=63).contains(&name.len())
        && name.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && name.parse::<std::net::Ipv4Addr>().is_err()
        && !name.starts_with("xn--")
        && !name.starts_with("sthree-")
        && !["-s3alias", "--ol-s3", ".mrap", "--x-s3", "--table-s3"]
            .iter()
            .any(|s| name.ends_with(s));
    valid.then_some(()).ok_or(Error::Invalid)
}
pub fn object_key(key: &str) -> Result<()> {
    (!key.is_empty() && key.len() <= 1024 && !key.chars().any(char::is_control))
        .then_some(())
        .ok_or(Error::Invalid)
}

#[derive(Debug, Clone)]
pub struct Blob {
    pub id: Uuid,
    pub size: i64,
    pub sha256: String,
    pub etag: String,
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn put(&self, input: ByteStream, limit: u64) -> Result<Blob>;
    async fn read(&self, blob: &Blob, offset: u64, length: u64) -> Result<ByteStream>;
    async fn verify(&self, blob: &Blob) -> Result<()>;
    async fn remove(&self, id: Uuid) -> Result<()>;
    async fn size(&self, id: Uuid) -> Result<Option<u64>>;
    async fn ready(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_resource_names() {
        for name in ["a..b", "a", "127.0.0.1", "BAD", "foo--x-s3"] {
            assert!(bucket_name(name).is_err());
        }
        assert!(bucket_name("data.backup-1").is_ok());
        assert!(object_key("../raw/2026").is_ok());
        assert!(object_key(&"é".repeat(513)).is_err());
    }
    #[test]
    fn role_boundaries() {
        assert!(Role::Admin.permits(Permission::Manage));
        assert!(!Role::ReadWrite.permits(Permission::Manage));
        assert!(!Role::ReadOnly.permits(Permission::Write));
    }
}
