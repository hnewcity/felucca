//! Embed the prebuilt React application; fail rather than ship stale assets.
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

fn collect(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("read admin assets") {
        let entry = entry.expect("admin asset");
        let path = entry.path();
        if entry.file_type().expect("asset type").is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("manifest directory");
    let app = Path::new(&manifest).join("../../apps/admin");
    let root = app.join("dist");
    let instructions = "Build the management UI first: npm --prefix apps/admin ci && npm --prefix apps/admin run build";
    let stamp = fs::read_to_string(root.join(".source-hash")).expect(instructions);
    let mut inputs = Vec::new();
    for directory in ["src", "public", "build"] {
        let path = app.join(directory);
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&path, &mut inputs);
    }
    for name in [
        "package.json",
        "package-lock.json",
        "index.html",
        "vite.config.js",
    ] {
        inputs.push(app.join(name));
    }
    inputs.sort();
    let mut hash = Sha256::new();
    for file in inputs {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file
            .strip_prefix(&app)
            .expect("frontend source")
            .to_str()
            .expect("UTF-8 path")
            .replace('\\', "/");
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(fs::read(file).expect("read frontend source"));
        hash.update([0]);
    }
    assert_eq!(
        stamp.trim(),
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        "Management UI is stale. {instructions}"
    );
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    let mut source = String::from(
        "fn embedded(path: &str) -> Option<(&'static str, &'static [u8])> {\nSome(match path {\n",
    );
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file
            .strip_prefix(&root)
            .expect("asset root")
            .to_str()
            .expect("UTF-8 asset path")
            .replace('\\', "/");
        let path = if relative == "index.html" {
            "/".to_owned()
        } else {
            format!("/{relative}")
        };
        let mime = match file
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
        {
            "html" => "text/html; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "svg" => "image/svg+xml",
            "woff2" => "font/woff2",
            "txt" => "text/plain; charset=utf-8",
            _ => continue,
        };
        let absolute = file.canonicalize().expect("asset absolute path");
        source.push_str(&format!(
            "{path:?} => ({mime:?}, include_bytes!({:?})),\n",
            absolute.to_str().expect("UTF-8 asset path")
        ));
    }
    source.push_str("_ => return None,\n})\n}\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").expect("output directory")).join("admin_assets.rs"),
        source,
    )
    .expect("write embedded assets");
}
