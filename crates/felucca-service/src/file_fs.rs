//! WebDAV filesystem confined to an opened directory capability.
//! Every operation, including COPY/MOVE destinations, resolves through this
//! handle. A canonicalize-then-open check would race with symlink replacement.
use bytes::{Buf, Bytes};
use cap_std::fs::Dir;
use dav_server::{
    davpath::DavPath,
    fs::{
        DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsResult, FsStream,
        OpenOptions, ReadDirMeta,
    },
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub(crate) struct ConfinedFs {
    root: Arc<Dir>,
    read_only: bool,
}
impl ConfinedFs {
    pub(crate) fn from_dir(root: Dir, read_only: bool) -> Self {
        Self {
            root: Arc::new(root),
            read_only,
        }
    }
    pub fn open_root(path: &Path, read_only: bool) -> io::Result<Self> {
        Ok(Self {
            root: Arc::new(Dir::open_ambient_dir(path, cap_std::ambient_authority())?),
            read_only,
        })
    }
    fn writable(&self) -> FsResult<()> {
        if self.read_only {
            Err(FsError::Forbidden)
        } else {
            Ok(())
        }
    }
    fn regular_file(&self, path: &Path, options: OpenOptions) -> FsResult<cap_std::fs::File> {
        if options.write
            || options.append
            || options.create
            || options.create_new
            || options.truncate
        {
            self.writable()?;
        }
        match self.root.metadata(path) {
            Ok(meta) if !meta.is_file() => return Err(FsError::Forbidden),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(error(e)),
            _ => {}
        }
        let mut open = cap_std::fs::OpenOptions::new();
        open.read(options.read)
            .write(options.write)
            .append(options.append)
            .truncate(options.truncate)
            .create(options.create)
            .create_new(options.create_new);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            // Also covers COPY source/destination races involving special files.
            open.custom_flags(libc::O_NONBLOCK).mode(0o600);
        }
        let file = self.root.open_with(path, &open).map_err(error)?;
        if !file.metadata().map_err(error)?.is_file() {
            return Err(FsError::Forbidden);
        }
        Ok(file)
    }
}
fn path(path: &DavPath) -> PathBuf {
    let path = path.as_rel_ospath();
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_owned()
    }
}
fn error(error: io::Error) -> FsError {
    match error.kind() {
        io::ErrorKind::NotFound => FsError::NotFound,
        io::ErrorKind::PermissionDenied => FsError::Forbidden,
        io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty => FsError::Exists,
        io::ErrorKind::StorageFull => FsError::InsufficientStorage,
        io::ErrorKind::InvalidInput
        | io::ErrorKind::IsADirectory
        | io::ErrorKind::NotADirectory => FsError::Forbidden,
        _ => FsError::GeneralFailure,
    }
}
fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> FsResult<T> + Send + 'static,
) -> FsFuture<'static, T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|_| FsError::GeneralFailure)?
    })
}

impl DavFileSystem for ConfinedFs {
    fn open<'a>(
        &'a self,
        dav: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        let this = self.clone();
        let path = path(dav);
        blocking(move || {
            let file = this.regular_file(&path, options)?;
            Ok(Box::new(DavIo(tokio::fs::File::from_std(file.into_std()))) as Box<dyn DavFile>)
        })
    }

    fn read_dir<'a>(
        &'a self,
        dav: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        let root = self.root.clone();
        let parent = path(dav);
        Box::pin(async move {
            let r = root.clone();
            let p = parent.clone();
            let entries = blocking(move || r.read_dir(p).map_err(error)).await?;
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::task::spawn_blocking(move || {
                for entry in entries {
                    let entry = entry.map_err(error).map(|entry| {
                        let name = entry.file_name();
                        Box::new(Entry {
                            name: name.as_encoded_bytes().to_vec(),
                            path: parent.join(name),
                            root: root.clone(),
                            follow: meta == ReadDirMeta::Data,
                        }) as Box<dyn DavDirEntry>
                    });
                    if tx.blocking_send(entry).is_err() {
                        break;
                    }
                }
            });
            Ok(Box::pin(futures_util::stream::unfold(rx, |mut rx| async {
                rx.recv().await.map(|entry| (entry, rx))
            })) as FsStream<Box<dyn DavDirEntry>>)
        })
    }
    fn metadata<'a>(&'a self, dav: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let root = self.root.clone();
        let path = path(dav);
        blocking(move || {
            Ok(Box::new(Meta::cap(root.metadata(path).map_err(error)?)) as Box<dyn DavMetaData>)
        })
    }
    fn symlink_metadata<'a>(&'a self, dav: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let root = self.root.clone();
        let path = path(dav);
        blocking(move || {
            Ok(
                Box::new(Meta::cap(root.symlink_metadata(path).map_err(error)?))
                    as Box<dyn DavMetaData>,
            )
        })
    }
    fn create_dir<'a>(&'a self, dav: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let path = path(dav);
        blocking(move || {
            this.writable()?;
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = cap_std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use cap_std::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            this.root.create_dir_with(path, &builder).map_err(error)
        })
    }
    fn remove_dir<'a>(&'a self, dav: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let path = path(dav);
        blocking(move || {
            this.writable()?;
            this.root.remove_dir(path).map_err(error)
        })
    }
    fn remove_file<'a>(&'a self, dav: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let path = path(dav);
        blocking(move || {
            this.writable()?;
            this.root.remove_file(path).map_err(error)
        })
    }
    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let (from, to) = (path(from), path(to));
        blocking(move || {
            this.writable()?;
            this.root.rename(from, &this.root, to).map_err(error)
        })
    }
    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        let this = self.clone();
        let (from, to) = (path(from), path(to));
        blocking(move || {
            this.writable()?;
            let mut source = this.regular_file(
                &from,
                OpenOptions {
                    read: true,
                    ..Default::default()
                },
            )?;
            // DAV removes an overwritten destination before reaching this
            // callback. Exclusive creation also rejects a raced symlink/hardlink
            // to the source instead of truncating that source through an alias.
            let mut target = this.regular_file(
                &to,
                OpenOptions {
                    write: true,
                    create_new: true,
                    ..Default::default()
                },
            )?;
            std::io::copy(&mut source, &mut target).map_err(error)?;
            target.sync_data().map_err(error)
        })
    }
}

struct Entry {
    root: Arc<Dir>,
    path: PathBuf,
    name: Vec<u8>,
    follow: bool,
}
impl DavDirEntry for Entry {
    fn name(&self) -> Vec<u8> {
        self.name.clone()
    }
    fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let root = self.root.clone();
        let path = self.path.clone();
        let follow = self.follow;
        blocking(move || {
            let meta = if follow {
                root.metadata(path)
            } else {
                root.symlink_metadata(path)
            }
            .map_err(error)?;
            Ok(Box::new(Meta::cap(meta)) as Box<dyn DavMetaData>)
        })
    }
}

#[derive(Debug, Clone)]
struct Meta {
    length: u64,
    directory: bool,
    file: bool,
    symlink: bool,
    modified: FsResult<SystemTime>,
    created: FsResult<SystemTime>,
}
impl Meta {
    fn cap(meta: cap_std::fs::Metadata) -> Self {
        Self {
            length: meta.len(),
            directory: meta.is_dir(),
            file: meta.is_file(),
            symlink: meta.file_type().is_symlink(),
            modified: meta.modified().map(|time| time.into_std()).map_err(error),
            created: meta.created().map(|time| time.into_std()).map_err(error),
        }
    }
    fn std(meta: std::fs::Metadata) -> Self {
        Self {
            length: meta.len(),
            directory: meta.is_dir(),
            file: meta.is_file(),
            symlink: meta.file_type().is_symlink(),
            modified: meta.modified().map_err(error),
            created: meta.created().map_err(error),
        }
    }
}
impl DavMetaData for Meta {
    fn len(&self) -> u64 {
        self.length
    }
    fn is_dir(&self) -> bool {
        self.directory
    }
    fn is_file(&self) -> bool {
        self.file
    }
    fn is_symlink(&self) -> bool {
        self.symlink
    }
    fn modified(&self) -> FsResult<SystemTime> {
        self.modified
    }
    fn created(&self) -> FsResult<SystemTime> {
        self.created
    }
}

#[derive(Debug)]
struct DavIo(tokio::fs::File);
impl DavFile for DavIo {
    fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        Box::pin(async move {
            Ok(Box::new(Meta::std(self.0.metadata().await.map_err(error)?))
                as Box<dyn DavMetaData>)
        })
    }
    fn write_buf(&mut self, mut buffer: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        Box::pin(async move {
            while buffer.has_remaining() {
                let count = self.0.write(buffer.chunk()).await.map_err(error)?;
                if count == 0 {
                    return Err(FsError::GeneralFailure);
                }
                buffer.advance(count);
            }
            Ok(())
        })
    }
    fn write_bytes(&mut self, bytes: Bytes) -> FsFuture<'_, ()> {
        Box::pin(async move { self.0.write_all(&bytes).await.map_err(error) })
    }
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
        Box::pin(async move {
            let mut buffer = vec![0; count.min(64 * 1024)];
            let size = self.0.read(&mut buffer).await.map_err(error)?;
            buffer.truncate(size);
            Ok(Bytes::from(buffer))
        })
    }
    fn seek(&mut self, position: io::SeekFrom) -> FsFuture<'_, u64> {
        Box::pin(async move { self.0.seek(position).await.map_err(error) })
    }
    fn flush(&mut self) -> FsFuture<'_, ()> {
        Box::pin(async move {
            self.0.flush().await.map_err(error)?;
            self.0.sync_data().await.map_err(error)
        })
    }
}

#[cfg(test)]
#[path = "file_fs_tests.rs"]
mod tests;
