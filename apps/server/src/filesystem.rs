//! Immutable blobs, read-only startup checks and offline recovery.
use async_trait::async_trait;
use datavault_core::{Blob, ByteStream, Error, Result, Storage};
use fs2::FileExt;
use futures_util::StreamExt;
use md5::Md5;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io, path::{Path, PathBuf}};
use tokio::{fs, io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt}};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

pub struct Filesystem {
    root: PathBuf,
    objects: PathBuf,
    temp: PathBuf,
    _lock: std::fs::File,
}

fn failure(e: io::Error) -> Error {
    tracing::error!(kind=?e.kind(), "filesystem operation failed");
    Error::Storage
}

async fn directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path).await.map_err(failure)?.file_type().is_dir() {
        return Err(Error::Storage);
    }
    Ok(())
}

async fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path).await.map_err(failure)?.sync_all().await.map_err(failure)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Filesystem {
    pub async fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).await.map_err(failure)?;
        directory(root).await?;
        let root = fs::canonicalize(root).await.map_err(failure)?;
        let lock_path = root.join(".lock");
        let lock = tokio::task::spawn_blocking(move || {
            if std::fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.file_type().is_file()) {
                return Err(Error::Storage);
            }
            let file = std::fs::OpenOptions::new().create(true).truncate(false)
                .read(true).write(true).open(lock_path).map_err(failure)?;
            FileExt::try_lock_exclusive(&file).map_err(failure)?;
            Ok(file)
        }).await.map_err(|_| Error::Storage)??;
        let objects = root.join("objects");
        let temp = root.join("tmp");
        for path in [&objects, &temp] {
            fs::create_dir_all(path).await.map_err(failure)?;
            directory(path).await?;
        }
        sync_dir(&root).await?;
        Ok(Self { root, objects, temp, _lock: lock })
    }

    /// Refuse to collect an unknown volume or recover against a different catalog.
    pub async fn bind_catalog(&self, id: Uuid, has_references: bool) -> Result<()> {
        let marker = self.root.join(".catalog");
        match fs::symlink_metadata(&marker).await {
            Ok(m) if m.file_type().is_file() => {
                let value = fs::read_to_string(marker).await.map_err(failure)?;
                if value != id.to_string() { return Err(Error::Storage); }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if has_references || fs::read_dir(&self.objects).await.map_err(failure)?.next_entry().await.map_err(failure)?.is_some() {
                    return Err(Error::Storage);
                }
                let mut f = fs::OpenOptions::new().create_new(true).write(true).open(marker).await.map_err(failure)?;
                f.write_all(id.to_string().as_bytes()).await.map_err(failure)?;
                f.sync_all().await.map_err(failure)?;
                sync_dir(&self.root).await?;
            }
            _ => return Err(Error::Storage),
        }
        Ok(())
    }

    pub async fn inspect(&self, live: &HashSet<Uuid>) -> Result<(u64,u64)> {
        for id in live {
            if self.size(*id).await?.is_none() { return Err(Error::Storage); }
        }
        let mut unreferenced = 0;
        let mut temporary = 0;
        for path in [&self.objects, &self.temp] {
            let mut entries = fs::read_dir(path).await.map_err(failure)?;
            while let Some(e) = entries.next_entry().await.map_err(failure)? {
                let Some(id) = e.file_name().to_str().and_then(|s| Uuid::parse_str(s).ok()) else { continue };
                if !e.file_type().await.map_err(failure)?.is_file() { return Err(Error::Storage); }
                if path == &self.temp { temporary += 1; }
                else if !live.contains(&id) { unreferenced += 1; }
            }
        }
        Ok((unreferenced,temporary))
    }

    /// Preserve unused UUID files in a fresh recovery directory, while both writer locks are held.
    /// A partially completed run keeps both moved and unmoved files recoverable.
    pub async fn archive_unused(&self, live: &HashSet<Uuid>) -> Result<PathBuf> {
        self.inspect(live).await?;
        let recovery = self.root.join("recovery");
        fs::create_dir_all(&recovery).await.map_err(failure)?;
        directory(&recovery).await?;
        let batch = recovery.join(Uuid::new_v4().to_string());
        fs::create_dir(&batch).await.map_err(failure)?;
        sync_dir(&self.root).await?;
        sync_dir(&recovery).await?;
        for (source, name) in [(&self.objects, "objects"), (&self.temp, "tmp")] {
            let destination = batch.join(name);
            fs::create_dir(&destination).await.map_err(failure)?;
            sync_dir(&batch).await?;
            let mut entries = fs::read_dir(source).await.map_err(failure)?;
            while let Some(entry) = entries.next_entry().await.map_err(failure)? {
                let filename = entry.file_name();
                let Some(id) = filename.to_str().and_then(|s| Uuid::parse_str(s).ok()) else {
                    continue;
                };
                if source == &self.objects && live.contains(&id) {
                    continue;
                }
                if !entry.file_type().await.map_err(failure)?.is_file() {
                    return Err(Error::Storage);
                }
                fs::rename(entry.path(), destination.join(filename)).await.map_err(failure)?;
            }
            sync_dir(&destination).await?;
            sync_dir(source).await?;
        }
        Ok(batch)
    }

    async fn write(&self, id: Uuid, temp: &Path, mut stream: ByteStream, limit: u64) -> Result<Blob> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(temp).await.map_err(failure)?;
        let mut size = 0u64;
        let mut sha = Sha256::new();
        let mut md5 = Md5::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::Invalid)?;
            size += chunk.len() as u64;
            if size > limit { return Err(Error::TooLarge); }
            sha.update(&chunk);
            md5.update(&chunk);
            file.write_all(&chunk).await.map_err(failure)?;
        }
        file.sync_all().await.map_err(failure)?;
        drop(file);
        fs::rename(temp, self.objects.join(id.to_string())).await.map_err(failure)?;
        sync_dir(&self.objects).await?;
        sync_dir(&self.temp).await?;
        Ok(Blob { id, size: size as i64, etag: hex::encode(md5.finalize()), sha256: hex::encode(sha.finalize()) })
    }
}

#[async_trait]
impl Storage for Filesystem {
    async fn put(&self, input: ByteStream, limit: u64) -> Result<Blob> {
        directory(&self.temp).await?;
        directory(&self.objects).await?;
        let id = Uuid::new_v4();
        let temp = self.temp.join(id.to_string());
        let result = self.write(id, &temp, input, limit).await;
        if result.is_err() { let _ = fs::remove_file(temp).await; }
        result
    }

    async fn size(&self, id: Uuid) -> Result<Option<u64>> {
        directory(&self.objects).await?;
        match fs::symlink_metadata(self.objects.join(id.to_string())).await {
            Ok(m) if m.file_type().is_file() => Ok(Some(m.len())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            _ => Err(Error::Storage),
        }
    }

    async fn remove(&self, id: Uuid) -> Result<()> {
        if self.size(id).await?.is_some() {
            fs::remove_file(self.objects.join(id.to_string())).await.map_err(failure)?;
            sync_dir(&self.objects).await?;
        }
        Ok(())
    }

    async fn read(&self, blob: &Blob, start: u64, length: u64) -> Result<ByteStream> {
        if start.checked_add(length).is_none_or(|end| end > blob.size as u64) { return Err(Error::Range); }
        if self.size(blob.id).await? != Some(blob.size as u64) { return Err(Error::Storage); }
        let mut file = fs::File::open(self.objects.join(blob.id.to_string())).await.map_err(failure)?;
        file.seek(io::SeekFrom::Start(start)).await.map_err(failure)?;
        Ok(Box::pin(ReaderStream::with_capacity(file.take(length), 64*1024)))
    }
    async fn verify(&self, blob: &Blob) -> Result<()> {
        let mut stream = self.read(blob, 0, blob.size as u64).await?;
        let mut sha = Sha256::new();
        while let Some(chunk) = stream.next().await { sha.update(chunk.map_err(failure)?); }
        if hex::encode(sha.finalize()) != blob.sha256 { return Err(Error::Checksum); }
        Ok(())
    }
    async fn ready(&self) -> Result<()> {
        let input: ByteStream = Box::pin(futures_util::stream::iter([Ok(b"1".as_slice().into())]));
        let probe = self.put(input, 1).await?;
        self.remove(probe.id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    fn stream(data: &'static [u8]) -> ByteStream {
        Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(data))]))
    }

    #[tokio::test]
    async fn interrupted_input_stays_invisible() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = Filesystem::open(dir.path()).await?;
        let interrupted = futures_util::stream::iter([
            Ok(Bytes::from_static(b"partial")),
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "disconnected")),
        ]);
        assert!(storage.put(Box::pin(interrupted), 1024).await.is_err());
        assert_eq!(storage.inspect(&HashSet::new()).await?, (0, 0));
        assert!(storage.inspect(&HashSet::from([Uuid::new_v4()])).await.is_err());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "subprocess fixture for crash_during_stream_preserves_committed_blob"]
    async fn crash_writer_fixture() -> anyhow::Result<()> {
        let root = PathBuf::from(std::env::var_os("DATAVAULT_TEST_CRASH_DIR").ok_or(Error::Invalid)?);
        let storage = Filesystem::open(&root).await?;
        let input = async_stream::try_stream! {
            yield Bytes::from(vec![1; 64 * 1024]);
            // Reaching the second poll proves the first chunk has been written.
            fs::write(root.join("writer-started"), b"ready").await?;
            std::future::pending::<()>().await;
        };
        storage.put(Box::pin(input), 1024 * 1024).await?;
        Ok(())
    }

    #[tokio::test]
    async fn crash_during_stream_preserves_committed_blob() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = Filesystem::open(dir.path()).await?;
        let committed = storage.put(stream(b"previous"), 1024).await?;
        drop(storage);
        let mut writer = tokio::process::Command::new(std::env::current_exe()?)
            .args(["--ignored", "--exact", "filesystem::tests::crash_writer_fixture"])
            .env("DATAVAULT_TEST_CRASH_DIR", dir.path())
            .kill_on_drop(true)
            .spawn()?;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !dir.path().join("writer-started").exists() {
                if let Some(status) = writer.try_wait()? {
                    anyhow::bail!("writer exited before the barrier: {status}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        }).await??;
        writer.kill().await?;
        let recovered = Filesystem::open(dir.path()).await?;
        assert_eq!(recovered.inspect(&HashSet::from([committed.id])).await?, (0, 1));
        recovered.verify(&committed).await?;
        Ok(())
    }

    #[tokio::test]
    async fn streams_repeated_chunks_with_a_bounded_buffer() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = Filesystem::open(dir.path()).await?;
        let block = Bytes::from(vec![7; 64 * 1024]);
        let input = futures_util::stream::iter((0..128).map(move |_| Ok(block.clone())));
        let size = 8 * 1024 * 1024;
        let blob = storage.put(Box::pin(input), size).await?;
        assert_eq!(blob.size as u64, size);
        storage.verify(&blob).await?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_a_symlink_in_the_storage_tree() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::os::unix::fs::symlink(outside.path(), dir.path().join("objects"))?;
        assert!(Filesystem::open(dir.path()).await.is_err());
        assert_eq!(outside.path().read_dir()?.count(), 0);
        Ok(())
    }
    #[tokio::test]
    async fn binary_range_restart_and_orphan_recovery() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let fs = Filesystem::open(dir.path()).await?;
        let catalog = Uuid::new_v4();
        fs.bind_catalog(catalog, false).await?;
        let blob = fs.put(stream(b"abcdef"), 6).await?;
        assert_eq!(blob.etag, "e80b5017098950fc58aad83c8c14978e");
        let orphan = fs.put(stream(b""), 6).await?;
        assert!(fs.put(stream(b"excess"), 2).await.is_err());
        assert!(Filesystem::open(dir.path()).await.is_err());
        drop(fs);
        let fs = Filesystem::open(dir.path()).await?;
        assert!(fs.bind_catalog(Uuid::new_v4(), true).await.is_err());
        fs.bind_catalog(catalog, true).await?;
        assert_eq!(fs.inspect(&HashSet::from([blob.id])).await?, (1,0));
        fs.verify(&blob).await?;
        assert_eq!(fs.size(orphan.id).await?, Some(0));
        let staging_id = Uuid::new_v4();
        tokio::fs::write(fs.temp.join(staging_id.to_string()), b"interrupted").await?;
        let archive = fs.archive_unused(&HashSet::from([blob.id])).await?;
        assert_eq!(fs.inspect(&HashSet::from([blob.id])).await?, (0,0));
        assert_eq!(tokio::fs::read(archive.join("tmp").join(staging_id.to_string())).await?, b"interrupted");
        assert!(archive.join("objects").join(orphan.id.to_string()).is_file());
        fs.verify(&blob).await?;
        let mut range = fs.read(&blob, 1, 3).await?;
        assert_eq!(range.next().await.transpose()?, Some(Bytes::from_static(b"bcd")));
        assert!(range.next().await.is_none());
        drop(range);
        tokio::fs::write(fs.objects.join(blob.id.to_string()), b"broken").await?;
        assert!(matches!(fs.verify(&blob).await, Err(Error::Checksum)));
        Ok(())
    }
}
