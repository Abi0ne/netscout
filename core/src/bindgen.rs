//! The `uniffi-bindgen` binary target (proc-macro mode).
//!
//! Built by `cargo build` and invoked by the build scripts to emit platform
//! bindings from the compiled library's embedded metadata:
//!
//! ```text
//! netscout-core-bindgen generate --library <lib> --language swift  --out-dir <dir>
//! netscout-core-bindgen generate --library <lib> --language kotlin --out-dir <dir>
//! ```

fn main() {
    uniffi::uniffi_bindgen_main();
}
