fn main() {
    #[cfg(feature = "generate-header")]
    generate_header();
}

#[cfg(feature = "generate-header")]
fn generate_header() {
    use std::path::PathBuf;

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out = PathBuf::from(&crate_dir)
        .parent()   // crates/
        .unwrap()
        .parent()   // workspace root
        .unwrap()
        .join("include")
        .join("cm108.h");

    let config = cbindgen::Config::from_file(
        PathBuf::from(&crate_dir).join("cbindgen.toml"),
    )
    .unwrap_or_default();

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen failed")
        .write_to_file(out);
}
