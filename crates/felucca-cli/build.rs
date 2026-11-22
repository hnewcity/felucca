fn main() {
    let target = std::env::var("TARGET").expect("Cargo supplies TARGET");
    println!("cargo:rustc-env=FELUCCA_BUILD_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
