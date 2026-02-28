# User Feedback Log

## 2026-02-08
- User requested research on pure-Rust JPEG XL decoder crates (jxl-oxide, jxl-rs) for potential integration into zencodecs
- User requested wasm32-wasip1 binary size comparison between jxl-oxide 0.12.5 and jxl-rs 0.3.0. Result: jxl-oxide is significantly larger (~66% more code weight). Test branch: test/jxl-oxide-size

## 2026-02-08: Implement HDR/color capabilities in ravif
User requested implementing the plan to expose HDR and wide gamut capabilities through ravif's builder API.
- 2026-02-25: User requested research on rgb, palette, and image crate pixel/color type abstractions for Pixel trait design.
- 2026-02-26: User requested updating `/home/lilith/work/zenjxl/src/zencodec.rs` to the new zencodec-types trait API. All 10 tests pass.
- 2026-02-27: User requested updating ImageInfo field accesses across 5 repos for zencodec-types SourceColor/EmbeddedMetadata restructure. Changes needed only in zencodecs (zcimg/src/info.rs, zcimg/src/process.rs, examples/icc_roundtrip.rs). zenavif/zenwebp/zengif/heic had no zencodec_types::ImageInfo field accesses to update.
- 2026-02-27: Phase 3d (PixelBuffer migration) — updated zencodecs-internal code for PixelBuffer-based DecodeOutput API. Codec adapters use `from_pixel_data()` bridge. All `into_*8()` callers use `.as_imgref()`. Pipeline unified to PixelBuffer in both resize/no-resize branches.
- 2026-02-27: User feedback on PixelBuffer/PixelDescriptor ergonomics plan:
  - Want BGRA-with-known-opaque-alpha optimization: a way to assert all alpha=255 so algorithms can skip alpha reads or treat as opaque, invalidated on mutation. Different from BGRX (padding, undefined) — this is "valid alpha that happens to be 255". Could be `AlphaMode::Opaque` or similar.
  - Requested PixelFormat enum, Display impls, ChannelType predicates, with_descriptor safety, reinterpret(), per-field setters, from_pixels_erased, copy_to_contiguous_bytes — all implemented in zencodec-types.
