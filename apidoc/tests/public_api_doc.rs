//! Public-API surface snapshots for the PARENT workspace (docs/public-api/).
//! Shared implementation + format docs: the `zenutils-apidoc` crate.
//!
//! Discovered library crates: zenpipe, zencodecs, zenfilters, zenlayout
//! (zenpipe-cmd is a binary; zeneditor is `publish = false`).
#[test]
fn public_api_surface_docs_are_current() {
    zenutils_apidoc::ApiDoc::new()
        .workspace_dir("..")
        // Include the consumer job, native nodes, sync/Tokio I/O and schema APIs.
        // Experimental tuner/face models and local-only node definitions are separate.
        .pinned_features("zenpipe", "job,job-ultrahdr,nodes-all,io-tokio,json-schema")
        // Include codec, CMS, JPEG 2000, metadata and explicit HDR rendering.
        .pinned_features(
            "zencodecs",
            "all,jp2-decode,metadata,tonemap,checked-transcode-async",
        )
        .run();
}
