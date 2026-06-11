//! Public-API surface snapshots for the PARENT workspace (docs/public-api/).
//! Shared implementation + format docs: the `zenutils-apidoc` crate.
//!
//! Discovered library crates: zenpipe, zencodecs, zenfilters, zenlayout
//! (zenpipe-cmd is a binary; zeneditor is `publish = false`).
#[test]
fn public_api_surface_docs_are_current() {
    zenutils_apidoc::ApiDoc::new()
        .workspace_dir("..")
        // zenpipe --all-features does not build (stub codec features), so its
        // full-feature default build is the supported surface: no features file.
        .no_extra_section("zenpipe")
        // zencodecs `--features all` does not build (gainmap drift); snapshot
        // the documented-good combo instead.
        .pinned_features("zencodecs", "jxl-encode,cms")
        .run();
}
