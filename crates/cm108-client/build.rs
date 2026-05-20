fn main() {
    // Generate include/cm108.h from the #[no_mangle] extern "C" surface in ffi.rs.
    // Active once Phase 4 FFI wrappers are implemented (--features generate-header).
    #[cfg(feature = "generate-header")]
    {
        let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let config = cbindgen::Config::from_file(
            std::path::Path::new(&crate_dir).join("cbindgen.toml"),
        )
        .unwrap_or_default();
        cbindgen::Builder::new()
            .with_crate(crate_dir)
            .with_config(config)
            .generate()
            .expect("cbindgen failed")
            .write_to_file("../../include/cm108.h");
    }
}
