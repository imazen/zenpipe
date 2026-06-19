//! HDR → SDR tone mapping at the transcode seam (feature `tonemap`).
//!
//! When a transcode decodes an HDR (PQ/HLG) source and re-encodes to a target
//! that has no HDR carrier, the HDR-tagged samples would otherwise be handed to
//! an SDR encoder's format negotiation and reinterpreted/clamped as sRGB —
//! wrong pixels, no error. This module detects that case and tone-maps to
//! sRGB8 first, using [`zentone`]'s BT.2408 EETF (the ITU-R reference HDR→SDR
//! operator).
//!
//! ## Anchor
//!
//! sRGB white (code 1.0 / 255) maps to **203 nits** — the BT.2408 HDR/SDR
//! reference white, the same `1.0 = 203 nit` diffuse-white anchor used across
//! `ultrahdr-core` / `heic` / `zenavif` / `zenjxl` (operator + anchor confirmed
//! by the maintainer, 2026-06-14). Content peak comes from the source
//! `MaxCLL`; absent that, the HDR10 baseline of 1000 nits is assumed.
//!
//! ## Scope (conservative by design)
//!
//! Fires only when **all** hold, so it never regresses an HDR-preserving path:
//! - the decoded buffer is an **f32** HDR buffer tagged `Pq`/`Hlg` (what zen
//!   decoders emit, e.g. zenavif `RGBF32`/`RGBAF32` with a PQ/HLG transfer);
//! - the target is a **verifiably SDR-only** format ([`target_is_sdr_only`]).
//!
//! HDR-capable (AVIF/JXL/HEIC) and ambiguous (PNG cICP, TIFF, float) targets
//! pass through unchanged so their HDR is preserved.

use alloc::vec::Vec;

use whereat::at;
use zencodec::{ImageFormat, Metadata};
use zenpixels::{ContentLightLevel, DiffuseWhite, PixelBuffer, PixelDescriptor, TransferFunction};
use zentone::pipeline::{
    tonemap_hlg_rgba_row_simd, tonemap_hlg_row_simd, tonemap_pq_to_srgb8_rgba_row_simd,
    tonemap_pq_to_srgb8_row_simd,
};
use zentone::{Bt2408Tonemapper, TonemapScratch};

use crate::CodecError;
use crate::error::Result;

/// sRGB white (1.0 / 255) maps to this display luminance — BT.2408 reference
/// white, the universal `1.0 = 203 nit` anchor of the zen HDR contract.
const SDR_DISPLAY_PEAK_NITS: f32 = 203.0;

/// Content peak assumed when the source carries no `MaxCLL` — the HDR10
/// baseline mastering peak.
const DEFAULT_CONTENT_PEAK_NITS: f32 = 1000.0;

/// Target formats with no HDR carrier (8-bit SDR), which therefore need an
/// HDR→SDR tone-map before encode.
///
/// Conservative: only formats verified SDR-only — `WebP` (`caps.hdr() == false`)
/// and `Jpeg`/`Gif` (8-bit, no HDR signaling). HDR-capable (`Avif`/`Jxl`/`Heic`)
/// and ambiguous (`Png` cICP, `Tiff`, float formats) targets return `false` and
/// pass the HDR buffer through unchanged.
///
// TODO: replace with a codec-caps query (`caps().hdr()`) once zencodecs can
// report per-format capabilities without instantiating an encoder.
fn target_is_sdr_only(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    )
}

/// Fallback content peak (nits) when the pixels can't be measured: the source
/// `MaxCLL` if present and non-zero, else the HDR10 baseline. Measurement
/// ([`measure_pq_content_peak`]) is preferred — see [`tonemap_to_srgb8`].
fn content_peak_nits(metadata: &Metadata) -> f32 {
    metadata
        .content_light_level
        .map(|c| c.max_content_light_level)
        .filter(|&m| m > 0)
        .map_or(DEFAULT_CONTENT_PEAK_NITS, f32::from)
}

/// Measure the PQ content peak (MaxCLL, in nits) from the actual pixels —
/// CTA-861.3 defines MaxCLL as a *measured* quantity, so the decoded pixels are
/// ground truth where a forwarded metadata field may be absent or stale.
///
/// PQ-decode the RGB channels to relative-linear (the PQ EOTF normalizes so
/// `1.0 = 10000 nits`) into a SIMD-aligned f32 buffer, then reduce with
/// [`ContentLightLevel::measure`] anchored at 10000. Returns `None` if the
/// measurement can't run (the caller then falls back to forwarded metadata).
///
/// HLG is **not** measured here: it is scene-referred, so its peak is
/// display-dependent (it needs the OOTF + a target display) rather than a
/// property of the samples alone.
fn measure_pq_content_peak(
    buffer: &PixelBuffer,
    width: usize,
    height: usize,
    src_bpp: usize,
    channels: usize,
) -> Option<f32> {
    let (w, h) = (width as u32, height as u32);
    // Aligned f32 destination so `measure`'s `&[u8] -> &[f32]` cast is sound.
    let mut lin = PixelBuffer::new(w, h, PixelDescriptor::RGBF32_LINEAR);
    {
        let src = buffer.as_slice();
        let mut dst = lin.as_slice_mut();
        for y in 0..h {
            let src_row: &[f32] = bytemuck::cast_slice(&src.row(y)[..width * src_bpp]);
            let dst_row: &mut [f32] = bytemuck::cast_slice_mut(dst.row_mut(y));
            // Gather RGB (drop alpha — luminance only), then PQ EOTF in place.
            for x in 0..width {
                dst_row[x * 3] = src_row[x * channels];
                dst_row[x * 3 + 1] = src_row[x * channels + 1];
                dst_row[x * 3 + 2] = src_row[x * channels + 2];
            }
            linear_srgb::default::pq_to_linear_slice(dst_row);
        }
    }
    // PQ-linear is normalized so 1.0 = 10000 nits.
    let cll = ContentLightLevel::measure(lin.as_slice(), DiffuseWhite::new(10000.0))?;
    (cll.max_content_light_level > 0).then(|| f32::from(cll.max_content_light_level))
}

/// If `buffer` is an f32 HDR (PQ/HLG) buffer and `target` cannot carry HDR,
/// tone-map it to an sRGB8 buffer (BT.2408 EETF, 203-nit anchor) and return
/// `Some(Ok(sdr_buffer))`. Returns `None` for every passthrough case (not HDR,
/// not an f32 HDR buffer, or an HDR-capable/ambiguous target).
pub(crate) fn maybe_tonemap_hdr_to_sdr(
    buffer: &PixelBuffer,
    target: ImageFormat,
    metadata: &Metadata,
) -> Option<Result<PixelBuffer>> {
    let desc = buffer.descriptor();
    let transfer = desc.transfer();
    if !matches!(transfer, TransferFunction::Pq | TransferFunction::Hlg) {
        return None;
    }
    // f32 RGB/RGBA only (4 bytes per channel, 3 or 4 channels) — the
    // representation zen HDR decoders emit (zenavif `RGBF32`/`RGBAF32` tagged
    // PQ/HLG). Never mis-cast a non-float (e.g. u16) HDR buffer as f32.
    let channels = desc.channels();
    if desc.bytes_per_pixel() != channels * 4 || !(channels == 3 || channels == 4) {
        return None;
    }
    if !target_is_sdr_only(target) {
        return None;
    }
    Some(tonemap_to_srgb8(buffer, transfer, metadata))
}

fn tonemap_to_srgb8(
    buffer: &PixelBuffer,
    transfer: TransferFunction,
    metadata: &Metadata,
) -> Result<PixelBuffer> {
    let desc = buffer.descriptor();
    let w = buffer.width();
    let h = buffer.height();
    let width = w as usize;
    let height = h as usize;
    let has_alpha = desc.has_alpha();
    let src_bpp = desc.bytes_per_pixel();
    let out_channels = if has_alpha { 4 } else { 3 };
    let out_desc = if has_alpha {
        PixelDescriptor::RGBA8_SRGB
    } else {
        PixelDescriptor::RGB8_SRGB
    };

    // Measure the content peak from the actual pixels (PQ) and drive the EETF
    // with it; HLG (scene-referred) and measurement failures fall back to the
    // forwarded metadata MaxCLL / the HDR10 default.
    let content_max = if transfer == TransferFunction::Pq {
        measure_pq_content_peak(buffer, width, height, src_bpp, desc.channels())
            .unwrap_or_else(|| content_peak_nits(metadata))
    } else {
        content_peak_nits(metadata)
    };
    let tm = Bt2408Tonemapper::new(content_max, SDR_DISPLAY_PEAK_NITS);
    let mut scratch = TonemapScratch::new();
    let mut out = alloc::vec![0u8; width * height * out_channels];

    // HLG has no direct zentone `→ sRGB8` entry, so we route through a linear
    // sRGB f32 strip and OETF it. Scratch is allocated once and reused per row.
    let mut hlg_lin3: Vec<[f32; 3]> = Vec::new();
    let mut hlg_lin4: Vec<[f32; 4]> = Vec::new();
    let mut hlg_rgb_u8: Vec<[u8; 3]> = Vec::new();
    if transfer == TransferFunction::Hlg {
        if has_alpha {
            hlg_lin4 = alloc::vec![[0.0f32; 4]; width];
            hlg_lin3 = alloc::vec![[0.0f32; 3]; width];
            hlg_rgb_u8 = alloc::vec![[0u8; 3]; width];
        } else {
            hlg_lin3 = alloc::vec![[0.0f32; 3]; width];
        }
    }

    let src = buffer.as_slice();
    for y in 0..height {
        // Per-row, width pixels — correct regardless of source row stride.
        let src_row = &src.row(y as u32)[..width * src_bpp];
        let dst_row = &mut out[y * width * out_channels..(y + 1) * width * out_channels];
        match (transfer, has_alpha) {
            (TransferFunction::Pq, false) => {
                let src: &[[f32; 3]] = bytemuck::cast_slice(src_row);
                let dst: &mut [[u8; 3]] = bytemuck::cast_slice_mut(dst_row);
                tonemap_pq_to_srgb8_row_simd(&mut scratch, src, dst, &tm);
            }
            (TransferFunction::Pq, true) => {
                let src: &[[f32; 4]] = bytemuck::cast_slice(src_row);
                let dst: &mut [[u8; 4]] = bytemuck::cast_slice_mut(dst_row);
                tonemap_pq_to_srgb8_rgba_row_simd(&mut scratch, src, dst, &tm);
            }
            (TransferFunction::Hlg, false) => {
                let src: &[[f32; 3]] = bytemuck::cast_slice(src_row);
                let dst: &mut [[u8; 3]] = bytemuck::cast_slice_mut(dst_row);
                tonemap_hlg_row_simd(&mut scratch, src, &mut hlg_lin3, &tm, SDR_DISPLAY_PEAK_NITS);
                linear_srgb::default::linear_to_srgb_u8_slice(
                    hlg_lin3.as_flattened(),
                    dst.as_flattened_mut(),
                );
            }
            (TransferFunction::Hlg, true) => {
                let src: &[[f32; 4]] = bytemuck::cast_slice(src_row);
                let dst: &mut [[u8; 4]] = bytemuck::cast_slice_mut(dst_row);
                tonemap_hlg_rgba_row_simd(
                    &mut scratch,
                    src,
                    &mut hlg_lin4,
                    &tm,
                    SDR_DISPLAY_PEAK_NITS,
                );
                // OETF the RGB channels in one SIMD pass; alpha is linear, so it
                // is quantized straight (matches zentone's PQ-RGBA convention).
                for (dst3, px) in hlg_lin3.iter_mut().zip(hlg_lin4.iter()) {
                    *dst3 = [px[0], px[1], px[2]];
                }
                linear_srgb::default::linear_to_srgb_u8_slice(
                    hlg_lin3.as_flattened(),
                    hlg_rgb_u8.as_flattened_mut(),
                );
                for ((out_px, rgb), px) in
                    dst.iter_mut().zip(hlg_rgb_u8.iter()).zip(hlg_lin4.iter())
                {
                    out_px[0] = rgb[0];
                    out_px[1] = rgb[1];
                    out_px[2] = rgb[2];
                    out_px[3] = (px[3] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
            _ => unreachable!("caller guarantees a PQ/HLG transfer"),
        }
    }

    PixelBuffer::from_vec(out, w, h, out_desc).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "tonemap output buffer: {e}"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat PQ buffer at the 203-nit diffuse-white anchor tone-maps to a
    /// valid neutral SDR gray — the EETF ran, rather than the raw PQ codes
    /// being reinterpreted as sRGB.
    #[test]
    fn pq_midgray_tonemaps_to_plausible_sdr() {
        let (w, h) = (8u32, 4u32);
        // PQ code ~0.58 ≈ 203 nits mid-gray (per zentone's own example).
        let pixels = alloc::vec![[0.58f32, 0.58, 0.58]; (w * h) as usize];
        let bytes = bytemuck::cast_slice::<[f32; 3], u8>(&pixels).to_vec();
        let desc = PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq);
        let buffer = PixelBuffer::from_vec(bytes, w, h, desc).unwrap();

        let meta = Metadata::none();
        let out = maybe_tonemap_hdr_to_sdr(&buffer, ImageFormat::Jpeg, &meta)
            .expect("PQ + SDR-only target must tonemap")
            .expect("tonemap must succeed");

        assert_eq!(out.descriptor(), PixelDescriptor::RGB8_SRGB);
        assert_eq!(out.width(), w);
        assert_eq!(out.height(), h);
        let slice = out.as_slice();
        let row0 = slice.row(0);
        let v = row0[0];
        // Valid, non-black result — the EETF ran (raw PQ codes were not
        // reinterpreted as sRGB). Measure-first reads the content peak as the
        // 203-nit diffuse white, so it maps bright; no tight upper bound.
        assert!(v > 8, "PQ should map to a valid sRGB gray, got {v}");
        // Gray in → neutral gray out (R≈G≈B).
        assert!(row0[0].abs_diff(row0[1]) <= 2 && row0[1].abs_diff(row0[2]) <= 2);
    }

    /// Measure-first: the content peak comes from the pixels. A uniform
    /// 203-nit (diffuse-white) PQ buffer measures near 203 nits — not the
    /// 1000-nit default — so the EETF adapts to the actual content.
    #[test]
    fn measures_pq_content_peak_from_pixels() {
        let (w, h) = (8u32, 4u32);
        // PQ code ~0.58 ≈ 203 nits (per zentone's example).
        let pixels = alloc::vec![[0.58f32, 0.58, 0.58]; (w * h) as usize];
        let bytes = bytemuck::cast_slice::<[f32; 3], u8>(&pixels).to_vec();
        let desc = PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq);
        let buffer = PixelBuffer::from_vec(bytes, w, h, desc).unwrap();

        let peak = measure_pq_content_peak(&buffer, w as usize, h as usize, 12, 3)
            .expect("PQ peak should measure");
        assert!(
            (150.0..280.0).contains(&peak),
            "measured PQ peak {peak} nits should be ~203, well below the 1000-nit default"
        );
    }

    /// An HDR-capable target (AVIF) is left untouched — passthrough preserves
    /// the HDR buffer for the encoder.
    #[test]
    fn hdr_capable_target_passes_through() {
        let pixels = alloc::vec![[0.5f32, 0.5, 0.5]; 16];
        let bytes = bytemuck::cast_slice::<[f32; 3], u8>(&pixels).to_vec();
        let desc = PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq);
        let buffer = PixelBuffer::from_vec(bytes, 4, 4, desc).unwrap();
        assert!(
            maybe_tonemap_hdr_to_sdr(&buffer, ImageFormat::Avif, &Metadata::none()).is_none(),
            "HDR-capable target must pass through"
        );
    }

    /// An SDR source is never tone-mapped, whatever the target.
    #[test]
    fn sdr_source_passes_through() {
        let bytes = alloc::vec![128u8; 4 * 4 * 3];
        let buffer = PixelBuffer::from_vec(bytes, 4, 4, PixelDescriptor::RGB8_SRGB).unwrap();
        assert!(
            maybe_tonemap_hdr_to_sdr(&buffer, ImageFormat::Jpeg, &Metadata::none()).is_none(),
            "SDR source must pass through"
        );
    }
}
