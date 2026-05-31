//! Cross-codec metadata conformance suite.
//!
//! The per-codec `*_capability.rs` tests prove each codec's metadata handling
//! in isolation. This suite does the orthogonal job: it runs **one uniform
//! battery** against **every enabled codec** so divergence in the shared
//! `zencodec` metadata contract is visible at a glance and regressions in any
//! codec adapter are caught against the same yardstick.
//!
//! The battery exercises the interchange surface
//! `zencodec::Metadata` → encode → decode → `ImageInfo`:
//!
//!   - ICC profile blob (byte-equal for verbatim embedders; present-after-decode
//!     for re-encoding codecs like JXL)
//!   - EXIF blob survival (`metadata().exif`)
//!   - EXIF orientation tag → normalized `info.orientation`
//!   - `Metadata::with_orientation` → emitted → normalized `info.orientation`
//!   - XMP packet (marker preserved via `metadata().xmp`)
//!   - CICP color signaling (`source_color.cicp` round-trip)
//!   - Clean baseline: no EXIF/XMP in → none out
//!   - Robustness: a fully-populated `Metadata` never breaks encode/decode,
//!     even for metadata-less containers (GIF).
//!
//! # The contract is machine-checked in both directions
//!
//! Each codec declares a [`Support`] verdict per dimension:
//!
//!   - [`V::Ok`] — round-trips today; asserted to keep working (regression guard).
//!   - [`V::NotCarried`] — the container has no carrier for this; dropping is
//!     correct. Asserted to *not* appear (so a codec silently gaining a carrier
//!     trips the test and the table gets corrected).
//!   - [`V::Gap`] — *should* round-trip but doesn't yet. Asserted to currently
//!     *not* appear, with a tracking note. When the gap is fixed the value
//!     starts surviving, the test trips, and you promote `Gap → Ok`.
//!
//! This makes the [`codecs`] table the living, self-correcting spec for what
//! metadata survives each codec through the unified `EncodeRequest`/
//! `DecodeRequest` path — every cell is verified by a test in this file, and
//! the table cannot drift from reality without a red build.
//!
//! The gaps recorded below are tracked in the repo issue referenced by
//! [`GAP_TRACKING`].

use imgref::ImgVec;
use rgb::{Rgb, Rgba};
use zencodec::exif::Exif;
use zencodec::{Cicp, Orientation};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::PixelSlice;

/// Where the alignment gaps recorded in the [`codecs`] table are tracked.
const GAP_TRACKING: &str = "imazen/zenpipe#36 (cross-codec metadata alignment)";

// ───────────────────────────── fixtures ─────────────────────────────

fn rgb8_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| Rgb {
            r: (i % w) as u8,
            g: (i / w) as u8,
            b: 128,
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn rgba8_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let pixels: Vec<Rgba<u8>> = (0..w * h)
        .map(|i| Rgba {
            r: (i % w) as u8,
            g: (i / w) as u8,
            b: 128,
            a: 220,
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// A synthetic ICC blob. Verbatim embedders (JPEG/PNG/WebP/AVIF) store the
/// bytes untouched, so byte-equality is the property we test. (JXL validates
/// and re-encodes color, so it gets a *valid* profile instead — see
/// `icc_present_for_reencoding_codecs`.)
fn synthetic_icc(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&(len as u32).to_be_bytes());
    while v.len() < len {
        v.push((v.len() as u8).wrapping_mul(31));
    }
    v
}

/// Minimal little-endian TIFF/EXIF blob containing only an Orientation tag.
fn exif_with_orientation(value: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2a\x00"); // little-endian TIFF magic
    v.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
    v.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
    v.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation tag
    v.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    v.extend_from_slice(&1u32.to_le_bytes()); // count 1
    v.extend_from_slice(&(value as u32).to_le_bytes()); // inline value
    v.extend_from_slice(&0u32.to_le_bytes()); // next-IFD = 0
    v
}

/// ASCII copyright string for the cross-codec rights-retention tests. (UTF-8
/// EXIF text is exercised in zencodec's own `exif.rs` unit tests; here we test
/// *survival across codecs*, where ASCII is the robust common denominator.)
const COPYRIGHT: &str = "Copyright 2026 Imazen LLC. All rights reserved.";

/// Little-endian TIFF/EXIF blob with an inline Orientation (SHORT) and an
/// out-of-line ASCII Copyright (0x8298) tag — the two fields the metadata
/// retention work is built to preserve.
fn exif_with_copyright(orientation: u16, copyright: &str) -> Vec<u8> {
    let s = copyright.as_bytes();
    let count = (s.len() + 1) as u32; // NUL-terminated per TIFF ASCII
    // Layout: header(8) + entry_count(2) + 2 entries(24) + next_ifd(4) = 38.
    let data_offset: u32 = 8 + 2 + 24 + 4;

    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2a\x00"); // little-endian TIFF magic
    v.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
    v.extend_from_slice(&2u16.to_le_bytes()); // 2 entries (tag-sorted)
    // 0x0112 Orientation, SHORT, count 1, inline value.
    v.extend_from_slice(&0x0112u16.to_le_bytes());
    v.extend_from_slice(&3u16.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&(orientation as u32).to_le_bytes()); // LE: SHORT in low 2 bytes
    // 0x8298 Copyright, ASCII, out-of-line at data_offset.
    v.extend_from_slice(&0x8298u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&count.to_le_bytes());
    v.extend_from_slice(&data_offset.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // next-IFD = 0
    debug_assert_eq!(v.len() as u32, data_offset);
    v.extend_from_slice(s);
    v.push(0); // NUL terminator
    v
}

/// Parse an EXIF blob extracted from a decode, tolerating the `Exif\0\0` APP1
/// prefix that some carriers (JPEG) keep and others strip.
fn parse_exif(blob: &[u8]) -> Option<Exif<'_>> {
    let tiff = blob.strip_prefix(b"Exif\x00\x00").unwrap_or(blob);
    Exif::parse(tiff)
}

const XMP_MARKER: &str = "zencodec-conformance-marker";

fn xmp_packet() -> Vec<u8> {
    format!(
        "<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
<dc:identifier>{XMP_MARKER}</dc:identifier>\
</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    )
    .into_bytes()
}

/// A distinctive, non-default CICP so a survived value can't be confused with
/// an encoder's implicit sRGB default.
fn test_cicp() -> Cicp {
    Cicp::DISPLAY_P3
}

// ───────────────────────────── contract model ─────────────────────────────

/// Per-dimension verdict. See the module docs for the two-directional
/// assertion semantics.
#[derive(Clone, Copy, PartialEq)]
enum V {
    /// Round-trips today — asserted to keep working.
    Ok,
    /// Container has no carrier; dropping is correct — asserted absent.
    NotCarried,
    /// Should round-trip but doesn't yet — asserted absent, with a note.
    Gap(&'static str),
}

/// How a codec treats an embedded ICC profile.
// `PresentReencoded` is only constructed when a re-encoding codec (JXL) is
// compiled in; under feature subsets without one, the variant is unused.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
enum Icc {
    /// Profile bytes survive verbatim (byte-equal round-trip).
    ByteEqual,
    /// Profile is parsed/re-encoded; only *presence* is guaranteed (JXL).
    PresentReencoded,
    /// No ICC carrier.
    NotCarried,
}

/// Metadata-handling contract for one codec through the unified path.
#[derive(Clone, Copy)]
struct Support {
    fmt: ImageFormat,
    name: &'static str,
    /// GIF only accepts RGBA8 through the modern `erase()` path.
    needs_rgba: bool,
    icc: Icc,
    exif_blob: V,
    /// EXIF orientation tag resolves to a non-Identity `info.orientation`.
    orient_from_exif: V,
    /// `Metadata::with_orientation` is emitted and resolves on decode.
    orient_from_field: V,
    xmp: V,
    cicp: V,
}

// Recurring gap descriptions (each maps to a line item in GAP_TRACKING).
const G_ORIENT_FIELD: &str = "Metadata::orientation not emitted to the format's carrier (only AVIF irot does); \
     needs the exif write path + per-codec emission";
const G_ORIENT_EXIF_NORM: &str = "carries the EXIF blob but does not normalize its orientation tag into info.orientation \
     (JPEG/WebP/AVIF do)";
const G_CICP_NATIVE: &str =
    "format has a native color-signaling box but Metadata::cicp is not wired through encode/decode";
const G_AVIF_EXIF_BLOB: &str =
    "raw EXIF blob is dropped (orientation is absorbed into irot, but metadata().exif is None)";

/// The set of codecs compiled into this build, each cfg-gated on the feature
/// that also enables its encoder. Verdicts below are the *observed* behavior
/// through `EncodeRequest`/`DecodeRequest` as of this revision; see
/// `print_support_matrix` to re-derive the live matrix.
#[allow(clippy::vec_init_then_push)] // pushes are cfg-gated; `vec![]` can't express that
fn codecs() -> Vec<Support> {
    #[allow(unused_mut)]
    let mut v: Vec<Support> = Vec::new();

    #[cfg(feature = "jpeg")]
    v.push(Support {
        fmt: ImageFormat::Jpeg,
        name: "jpeg",
        needs_rgba: false,
        icc: Icc::ByteEqual,
        exif_blob: V::Ok,
        orient_from_exif: V::Ok,
        orient_from_field: V::Gap(G_ORIENT_FIELD),
        xmp: V::Ok,
        cicp: V::NotCarried, // JFIF/EXIF JPEG has no standard CICP carrier; color via ICC
    });

    #[cfg(feature = "png")]
    v.push(Support {
        fmt: ImageFormat::Png,
        name: "png",
        needs_rgba: false,
        icc: Icc::ByteEqual,
        exif_blob: V::Ok, // eXIf chunk
        orient_from_exif: V::Gap(G_ORIENT_EXIF_NORM),
        orient_from_field: V::Gap(G_ORIENT_FIELD),
        xmp: V::Ok,
        cicp: V::Ok, // cICP chunk
    });

    #[cfg(feature = "webp")]
    v.push(Support {
        fmt: ImageFormat::WebP,
        name: "webp",
        needs_rgba: false,
        icc: Icc::ByteEqual,
        exif_blob: V::Ok,
        orient_from_exif: V::Ok,
        orient_from_field: V::Gap(G_ORIENT_FIELD),
        xmp: V::Ok,
        cicp: V::NotCarried, // VP8X has no CICP; color via ICC
    });

    #[cfg(feature = "gif")]
    v.push(Support {
        fmt: ImageFormat::Gif,
        name: "gif",
        needs_rgba: true,
        icc: Icc::NotCarried,
        exif_blob: V::NotCarried,
        orient_from_exif: V::NotCarried,
        orient_from_field: V::NotCarried,
        xmp: V::NotCarried,
        cicp: V::NotCarried,
    });

    #[cfg(feature = "avif-encode")]
    v.push(Support {
        fmt: ImageFormat::Avif,
        name: "avif",
        needs_rgba: false,
        icc: Icc::ByteEqual,
        exif_blob: V::Gap(G_AVIF_EXIF_BLOB),
        orient_from_exif: V::Ok,
        orient_from_field: V::Ok, // irot/imir
        xmp: V::Ok,
        cicp: V::Gap(G_CICP_NATIVE), // nclx colr box exists but Metadata::cicp isn't wired
    });

    #[cfg(feature = "jxl-encode")]
    v.push(Support {
        fmt: ImageFormat::Jxl,
        name: "jxl",
        needs_rgba: false,
        icc: Icc::PresentReencoded,
        exif_blob: V::Ok,
        orient_from_exif: V::Gap(G_ORIENT_EXIF_NORM),
        orient_from_field: V::Gap(G_ORIENT_FIELD),
        xmp: V::Ok,
        // Metadata::cicp now drives the JXL codestream enum color encoding
        // (zencodec::resolve_color_emit under ColorPolicy::Balanced); a redundant
        // ICC is dropped (JXL is cicp_safe_sole_carrier) and the decoder
        // synthesizes one back from the enum, so `Icc::PresentReencoded` holds.
        cicp: V::Ok,
    });

    v
}

// ───────────────────────── encode/decode harness ─────────────────────────

fn encode(c: &Support, meta: Metadata) -> Result<Vec<u8>, String> {
    let rgb = rgb8_image(64, 48);
    let rgba = rgba8_image(64, 48);
    let req = EncodeRequest::new(c.fmt)
        .with_quality(90.0)
        .with_metadata(meta);
    let out = if c.needs_rgba {
        let s: PixelSlice<'_, Rgba<u8>> = PixelSlice::from(rgba.as_ref());
        req.encode(s.erase(), false)
    } else {
        let s: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(rgb.as_ref());
        req.encode(s.erase(), false)
    };
    out.map(|o| o.into_vec()).map_err(|e| format!("{e}"))
}

fn decode(bytes: &[u8]) -> Result<zencodecs::DecodeOutput, String> {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .map_err(|e| format!("{e}"))
}

fn round_trip(c: &Support, meta: Metadata) -> zencodecs::DecodeOutput {
    let bytes = encode(c, meta).unwrap_or_else(|e| panic!("[{}] encode failed: {e}", c.name));
    decode(&bytes).unwrap_or_else(|e| panic!("[{}] decode failed: {e}", c.name))
}

/// Non-panicking round-trip for observers: a metadata dimension that can't even
/// be encoded+decoded back simply did *not* survive, which is a valid `false`
/// answer (e.g. JXL rejects a malformed ICC stream by design). Regressions are
/// still caught: a `V::Ok` dimension dropping to `false` trips its assertion.
fn try_round_trip(c: &Support, meta: Metadata) -> Option<zencodecs::DecodeOutput> {
    decode(&encode(c, meta).ok()?).ok()
}

// ───────────────────────── per-dimension observers ─────────────────────────

fn obs_icc_byte_equal(c: &Support) -> bool {
    let icc = synthetic_icc(256);
    try_round_trip(c, Metadata::none().with_icc(icc.clone()))
        .and_then(|d| d.info().source_color.icc_profile.clone())
        .map(|p| p.as_ref() == icc.as_slice())
        .unwrap_or(false)
}

fn obs_exif_blob(c: &Support) -> bool {
    try_round_trip(c, Metadata::none().with_exif(exif_with_orientation(1)))
        .map(|d| d.info().metadata().exif.is_some())
        .unwrap_or(false)
}

fn obs_orient_from_exif(c: &Support) -> bool {
    try_round_trip(c, Metadata::none().with_exif(exif_with_orientation(6)))
        .map(|d| d.info().orientation != Orientation::Identity)
        .unwrap_or(false)
}

fn obs_orient_from_field(c: &Support) -> bool {
    try_round_trip(c, Metadata::none().with_orientation(Orientation::Rotate90))
        .map(|d| d.info().orientation == Orientation::Rotate90)
        .unwrap_or(false)
}

fn obs_xmp(c: &Support) -> bool {
    try_round_trip(c, Metadata::none().with_xmp(xmp_packet()))
        .and_then(|d| d.info().metadata().xmp.clone())
        .map(|x| {
            core::str::from_utf8(&x)
                .map(|s| s.contains(XMP_MARKER))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn obs_cicp(c: &Support) -> bool {
    try_round_trip(c, Metadata::none().with_cicp(test_cicp()))
        .and_then(|d| d.info().source_color.cicp)
        .map(|got| got == test_cicp())
        .unwrap_or(false)
}

// ───────────────────────── verdict assertion ─────────────────────────

/// Assert the observed behavior matches the declared verdict — in both
/// directions, with messages that say exactly how to fix the table.
fn check(name: &str, dim: &str, observed: bool, v: V) {
    match v {
        V::Ok => assert!(
            observed,
            "REGRESSION: [{name}] {dim} used to round-trip and no longer does"
        ),
        V::NotCarried => assert!(
            !observed,
            "[{name}] {dim} unexpectedly survived — the format gained a carrier; \
             update the table (NotCarried → Ok)"
        ),
        V::Gap(note) => assert!(
            !observed,
            "[{name}] {dim} now round-trips! Known gap is FIXED — promote \
             V::Gap → V::Ok in codecs(). Gap was: {note} ({GAP_TRACKING})"
        ),
    }
}

// ───────────────────────────── dimension tests ─────────────────────────────

/// ICC byte-equality for verbatim embedders (JPEG/PNG/WebP/AVIF). Needs no CMS
/// — these codecs store the profile bytes untouched.
#[test]
fn icc_byte_equal_for_verbatim_embedders() {
    for c in codecs() {
        if c.icc == Icc::ByteEqual {
            check(c.name, "icc(byte-equal)", obs_icc_byte_equal(&c), V::Ok);
        }
    }
}

/// ICC for re-encoding codecs (JXL): a *valid* profile must survive as
/// *present* after decode, even though the bytes are not preserved. Gated on
/// `cms` because it needs a real sRGB profile from moxcms.
#[cfg(feature = "cms")]
#[test]
fn icc_present_for_reencoding_codecs() {
    use zencodecs::cms::srgb_icc_profile;
    for c in codecs() {
        if c.icc == Icc::PresentReencoded {
            let icc = srgb_icc_profile();
            let present = round_trip(&c, Metadata::none().with_icc(icc))
                .info()
                .source_color
                .icc_profile
                .as_ref()
                .map(|p| !p.is_empty())
                .unwrap_or(false);
            assert!(
                present,
                "[{}] a valid ICC profile must survive (as present) through re-encode",
                c.name
            );
        }
    }
}

/// Codecs declaring `NotCarried` for ICC must report no profile.
#[test]
fn icc_absent_when_not_carried() {
    for c in codecs() {
        if c.icc == Icc::NotCarried {
            let icc = synthetic_icc(256);
            let got = round_trip(&c, Metadata::none().with_icc(icc))
                .info()
                .source_color
                .icc_profile
                .is_some();
            assert!(
                !got,
                "[{}] declares no ICC carrier but a profile survived; update the table",
                c.name
            );
        }
    }
}

#[test]
fn exif_blob_survival() {
    for c in codecs() {
        check(c.name, "exif_blob", obs_exif_blob(&c), c.exif_blob);
    }
}

#[test]
fn orientation_from_exif_tag() {
    for c in codecs() {
        check(
            c.name,
            "orient_from_exif",
            obs_orient_from_exif(&c),
            c.orient_from_exif,
        );
    }
}

#[test]
fn orientation_from_metadata_field() {
    for c in codecs() {
        check(
            c.name,
            "orient_from_field",
            obs_orient_from_field(&c),
            c.orient_from_field,
        );
    }
}

#[test]
fn xmp_marker_survival() {
    for c in codecs() {
        check(c.name, "xmp", obs_xmp(&c), c.xmp);
    }
}

#[test]
fn cicp_color_signaling() {
    for c in codecs() {
        check(c.name, "cicp", obs_cicp(&c), c.cicp);
    }
}

// ───────────────────────── copyright (rights) retention ─────────────────────────

/// The end-to-end rights-retention contract: an EXIF Copyright tag survives a
/// round-trip and is parseable back into the same string via
/// `zencodec::exif::Exif`. Asserted for every codec whose EXIF blob round-trips
/// (`exif_blob == V::Ok`); the verdict table already governs which those are.
#[test]
fn copyright_tag_survives_round_trip() {
    for c in codecs() {
        if c.exif_blob != V::Ok {
            continue;
        }
        let exif = exif_with_copyright(1, COPYRIGHT);
        let out = round_trip(&c, Metadata::none().with_exif(exif));
        let info = out.info();
        let meta = info.metadata();
        let blob = meta
            .exif
            .as_ref()
            .unwrap_or_else(|| panic!("[{}] EXIF blob lost (declared V::Ok)", c.name));
        let parsed = parse_exif(blob)
            .unwrap_or_else(|| panic!("[{}] round-tripped EXIF unparseable", c.name));
        let copyright = parsed
            .copyright()
            .unwrap_or_else(|| panic!("[{}] Copyright tag absent after round-trip", c.name));
        assert!(
            copyright.contains(COPYRIGHT),
            "[{}] Copyright string corrupted: got {copyright:?}",
            c.name
        );
    }
}

// ───────────────────────── cross-codec transcode ─────────────────────────

/// Forward what a decode preserved into a fresh encode of another format —
/// the metadata side of a transcode (`decode A → re-encode B`).
fn transcode_forward(
    src: &Support,
    dst: &Support,
    meta: Metadata,
) -> Option<zencodecs::DecodeOutput> {
    let decoded = decode(&encode(src, meta).ok()?).ok()?;
    let fwd = {
        let info = decoded.info();
        let m = info.metadata();
        let mut fwd = Metadata::none().with_orientation(info.orientation);
        if let Some(e) = &m.exif {
            fwd = fwd.with_exif(e.clone());
        }
        if let Some(x) = &m.xmp {
            fwd = fwd.with_xmp(x.clone());
        }
        if let Some(icc) = &info.source_color.icc_profile {
            fwd = fwd.with_icc(icc.clone());
        }
        fwd
    };
    decode(&encode(dst, fwd).ok()?).ok()
}

/// Copyright must survive transcoding *between* any two EXIF-carrying codecs —
/// the core "transfer copyright tags between formats" guarantee. Every ordered
/// pair (src ≠ dst) among the `exif_blob == V::Ok` codecs is exercised.
#[test]
fn transcode_transfers_copyright_between_exif_codecs() {
    let all = codecs();
    let carriers: Vec<&Support> = all.iter().filter(|c| c.exif_blob == V::Ok).collect();
    for src in &carriers {
        for dst in &carriers {
            if src.name == dst.name {
                continue;
            }
            let exif = exif_with_copyright(1, COPYRIGHT);
            let out = transcode_forward(src, dst, Metadata::none().with_exif(exif))
                .unwrap_or_else(|| panic!("[{}→{}] transcode failed", src.name, dst.name));
            let info = out.info();
            let meta = info.metadata();
            let blob = meta
                .exif
                .as_ref()
                .unwrap_or_else(|| panic!("[{}→{}] EXIF lost in transcode", src.name, dst.name));
            let copyright = parse_exif(blob)
                .and_then(|e| e.copyright())
                .unwrap_or_else(|| {
                    panic!(
                        "[{}→{}] Copyright absent after transcode",
                        src.name, dst.name
                    )
                });
            assert!(
                copyright.contains(COPYRIGHT),
                "[{}→{}] Copyright corrupted: {copyright:?}",
                src.name,
                dst.name
            );
        }
    }
}

/// Predict whether `info.orientation == Rotate90` survives `src → dst`
/// transcoding given [`transcode_forward`] carries *both* the normalized
/// orientation field and the source's preserved EXIF blob.
///
/// The destination recovers orientation by either route:
///   - **field route** — dst emits `Metadata::orientation` to its carrier and
///     re-reads it (`orient_from_field`), and the source supplied a non-Identity
///     field (it normalized the input EXIF: `orient_from_exif`).
///   - **blob route** — the source kept the orientation-bearing EXIF blob
///     (`exif_blob`) and dst *normalizes* an incoming orientation tag
///     (`orient_from_exif`). Note dst need not *preserve* the blob — AVIF reads
///     orientation from an incoming EXIF blob (`orient_from_exif == Ok`) yet
///     drops the raw blob (`exif_blob == Gap`), and orientation still transfers.
fn predict_orientation_transfer(src: &Support, dst: &Support) -> bool {
    let src_field = src.orient_from_exif == V::Ok; // src.info.orientation is set
    let src_blob = src.exif_blob == V::Ok; // orientation tag survives in forwarded blob
    let via_field = dst.orient_from_field == V::Ok && src_field;
    let via_blob = dst.orient_from_exif == V::Ok && src_blob;
    via_field || via_blob
}

/// Full orientation-transfer matrix. For every ordered codec pair, transcoding
/// an EXIF-orientation=6 image must preserve orientation exactly when
/// [`predict_orientation_transfer`] says it should. This pins the real
/// cross-codec behavior — orientation transfers `jpeg→{webp,avif}` (blob/field
/// routes) but is lost `jpeg→{png,jxl}` (carry the blob but don't normalize it,
/// and don't emit the field — see `G_ORIENT_EXIF_NORM` / `G_ORIENT_FIELD`).
#[test]
fn transcode_orientation_transfer_matches_carrier_support() {
    let all = codecs();
    for src in &all {
        for dst in &all {
            if src.name == dst.name {
                continue;
            }
            let out = transcode_forward(
                src,
                dst,
                Metadata::none().with_exif(exif_with_orientation(6)),
            )
            .unwrap_or_else(|| panic!("[{}→{}] transcode failed", src.name, dst.name));
            let preserved = out.info().orientation == Orientation::Rotate90;
            assert_eq!(
                preserved,
                predict_orientation_transfer(src, dst),
                "[{}→{}] orientation-transfer behavior changed — re-derive the verdict \
                 table and predictor ({GAP_TRACKING})",
                src.name,
                dst.name,
            );
        }
    }
}

// ───────────────────────── cross-cutting contracts ─────────────────────────

/// No EXIF/XMP requested → none surfaced. (ICC/CICP may be defaulted by an
/// encoder since color is intrinsic, so they are not asserted here.)
#[test]
fn clean_baseline_emits_no_exif_or_xmp() {
    for c in codecs() {
        let out = round_trip(&c, Metadata::none());
        let info = out.info();
        let meta = info.metadata();
        assert!(
            meta.exif.is_none(),
            "[{}] emitted EXIF for a metadata-free encode",
            c.name
        );
        assert!(
            meta.xmp.is_none(),
            "[{}] emitted XMP for a metadata-free encode",
            c.name
        );
    }
}

/// A fully-populated `Metadata` of *valid* fields must never break encode or
/// decode — including for containers with no metadata carriers (GIF), which
/// must silently drop what they can't represent rather than erroring.
///
/// A valid ICC profile is included only under `cms` (it needs a real sRGB
/// profile from moxcms). The synthetic ICC used elsewhere is intentionally
/// malformed — JXL correctly rejects malformed ICC streams at decode, so it is
/// *not* "valid metadata" and is out of scope for this robustness contract.
#[test]
fn full_metadata_never_breaks_round_trip() {
    for c in codecs() {
        #[allow(unused_mut)] // `meta` is only reassigned under `cms`
        let mut meta = Metadata::none()
            .with_exif(exif_with_orientation(6))
            .with_xmp(xmp_packet())
            .with_cicp(test_cicp())
            .with_orientation(Orientation::Rotate90);
        #[cfg(feature = "cms")]
        {
            meta = meta.with_icc(zencodecs::cms::srgb_icc_profile());
        }
        let bytes = encode(&c, meta)
            .unwrap_or_else(|e| panic!("[{}] full-metadata encode must not error: {e}", c.name));
        let out = decode(&bytes)
            .unwrap_or_else(|e| panic!("[{}] full-metadata decode must not error: {e}", c.name));
        assert_eq!(out.info().width, 64, "[{}] width preserved", c.name);
        assert_eq!(out.info().height, 48, "[{}] height preserved", c.name);
    }
}

/// Dimensions are preserved through a plain round-trip for every codec.
#[test]
fn dimensions_round_trip() {
    for c in codecs() {
        let out = round_trip(&c, Metadata::none());
        assert_eq!(out.info().width, 64, "[{}] width", c.name);
        assert_eq!(out.info().height, 48, "[{}] height", c.name);
    }
}

// ───────────────────────────── diagnostic ─────────────────────────────

/// Prints the observed (codec × dimension) support matrix. Always passes — run
/// with `--nocapture` to read it. Used to keep the [`codecs`] table honest; the
/// dimension tests above turn any divergence into a hard failure.
#[test]
fn print_support_matrix() {
    let cs = codecs();
    eprintln!();
    eprintln!(
        "{:<6} {:>5} {:>9} {:>11} {:>12} {:>4} {:>5}",
        "codec", "icc", "exif_blob", "orient_exif", "orient_field", "xmp", "cicp"
    );
    let b = |x: bool| if x { "yes" } else { " . " };
    for c in &cs {
        eprintln!(
            "{:<6} {:>5} {:>9} {:>11} {:>12} {:>4} {:>5}",
            c.name,
            b(obs_icc_byte_equal(c)),
            b(obs_exif_blob(c)),
            b(obs_orient_from_exif(c)),
            b(obs_orient_from_field(c)),
            b(obs_xmp(c)),
            b(obs_cicp(c)),
        );
    }
    eprintln!();
}
