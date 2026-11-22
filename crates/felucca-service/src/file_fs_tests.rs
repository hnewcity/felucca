use super::*;

struct Fixture {
    root: PathBuf,
    fs: ConfinedFs,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "felucca-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("public")).unwrap();
        let fs = ConfinedFs::open_root(&root.join("public"), false).unwrap();
        Self { root, fs }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn dav(text: &str) -> DavPath {
    DavPath::new(text).unwrap()
}
fn read() -> OpenOptions {
    OpenOptions {
        read: true,
        ..Default::default()
    }
}
fn write() -> OpenOptions {
    OpenOptions {
        write: true,
        create: true,
        truncate: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn regular_files_support_read_seek_copy_move_and_read_only_enforcement() {
    let fixture = Fixture::new();
    fixture.fs.create_dir(&dav("/dir")).await.unwrap();
    let mut file = fixture.fs.open(&dav("/dir/a"), write()).await.unwrap();
    file.write_bytes(Bytes::from_static(b"file contents"))
        .await
        .unwrap();
    file.flush().await.unwrap();
    drop(file);
    fixture
        .fs
        .copy(&dav("/dir/a"), &dav("/dir/b"))
        .await
        .unwrap();
    fixture
        .fs
        .rename(&dav("/dir/b"), &dav("/dir/c"))
        .await
        .unwrap();
    let mut file = fixture.fs.open(&dav("/dir/c"), read()).await.unwrap();
    assert_eq!(file.metadata().await.unwrap().len(), 13);
    file.seek(io::SeekFrom::Start(5)).await.unwrap();
    assert_eq!(file.read_bytes(64).await.unwrap(), "contents");
    let readonly = ConfinedFs::open_root(&fixture.root.join("public"), true).unwrap();
    assert!(readonly.open(&dav("/dir/c"), write()).await.is_err());
    assert!(readonly.remove_file(&dav("/dir/c")).await.is_err());
    assert!(
        readonly
            .rename(&dav("/dir/a"), &dav("/dir/d"))
            .await
            .is_err()
    );
    assert!(readonly.copy(&dav("/dir/a"), &dav("/dir/d")).await.is_err());
    assert!(readonly.create_dir(&dav("/new")).await.is_err());
    fixture.fs.remove_file(&dav("/dir/a")).await.unwrap();
    fixture.fs.remove_file(&dav("/dir/c")).await.unwrap();
    fixture.fs.remove_dir(&dav("/dir")).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_expose_or_modify_paths_outside_the_shared_root() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.root.join("private")).unwrap();
    std::fs::write(fixture.root.join("private/secret"), b"outside secret").unwrap();
    std::fs::write(fixture.root.join("public/regular"), b"regular").unwrap();
    symlink("../private", fixture.root.join("public/escape")).unwrap();
    symlink("../private/secret", fixture.root.join("public/link")).unwrap();
    symlink("regular", fixture.root.join("public/alias")).unwrap();
    assert!(
        fixture
            .fs
            .copy(&dav("/regular"), &dav("/alias"))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(fixture.root.join("public/regular")).unwrap(),
        b"regular"
    );
    for path in ["/link", "/escape/secret"] {
        assert!(fixture.fs.open(&dav(path), read()).await.is_err());
        assert!(fixture.fs.open(&dav(path), write()).await.is_err());
        assert!(fixture.fs.metadata(&dav(path)).await.is_err());
    }
    assert!(
        fixture
            .fs
            .copy(&dav("/regular"), &dav("/escape/copy"))
            .await
            .is_err()
    );
    assert!(
        fixture
            .fs
            .rename(&dav("/regular"), &dav("/escape/moved"))
            .await
            .is_err()
    );
    assert!(fixture.fs.create_dir(&dav("/escape/new")).await.is_err());
    assert!(
        fixture
            .fs
            .remove_file(&dav("/escape/secret"))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(fixture.root.join("private/secret")).unwrap(),
        b"outside secret"
    );
    assert_eq!(
        std::fs::read_dir(fixture.root.join("private"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_a_directory_with_a_symlink_during_requests_does_not_escape() {
    use std::{
        os::unix::fs::symlink,
        sync::atomic::{AtomicBool, Ordering},
    };
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.root.join("private")).unwrap();
    std::fs::write(fixture.root.join("private/data"), b"secret").unwrap();
    std::fs::create_dir(fixture.root.join("public/changing")).unwrap();
    std::fs::write(fixture.root.join("public/changing/data"), b"public").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let root = fixture.root.clone();
    let done = stop.clone();
    let racer = std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            std::fs::rename(root.join("public/changing"), root.join("public/held")).unwrap();
            symlink("../private", root.join("public/changing")).unwrap();
            std::thread::yield_now();
            std::fs::remove_file(root.join("public/changing")).unwrap();
            std::fs::rename(root.join("public/held"), root.join("public/changing")).unwrap();
        }
    });
    let mut escaped = false;
    for _ in 0..200 {
        if let Ok(mut file) = fixture.fs.open(&dav("/changing/data"), read()).await {
            escaped |= file.read_bytes(64).await.unwrap() != "public";
        }
    }
    stop.store(true, Ordering::Relaxed);
    racer.join().unwrap();
    assert!(!escaped, "a raced symlink exposed a file outside the root");
}
