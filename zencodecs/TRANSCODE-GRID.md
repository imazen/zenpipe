# Transcode grid — source × dest, to a zensim-A `QualityTarget`

**Goal.** `transcode_to_quality(data, dest, target, ..)` accepts *any* decodable
source and produces *any* encodable dest at the **smallest byte size that meets a
zensim Profile-A `QualityTarget`**, using the best method for that (source, dest)
pair. **One-shot** — predict the encoder knob and encode once; no bisection search.

Status today: only `JPEG→JPEG` and `JPEG→JXL` are wired; every other pair returns
`UnsupportedOperation`. This doc is the plan to fill the grid.

---

## The one insight that organizes the whole grid

For a transcode, the **dest column picks the *method*; the source row only supplies
the *reference pixels*, the *quality ceiling*, and a few *coefficient-domain
shortcuts*.**

- **Method ≈ a function of the dest.** Hitting a zensim-A target in format B is B's
  problem: ask B's *one-shot picker* for the knob. Whether the pixels came from a
  JPEG or a PNG doesn't change *how* you drive B's encoder — only what you feed it.
- **The source sets the ceiling.** You can't make the output better than the input.
  A q40 JPEG can't be re-encoded to zensim-A 95 vs the *original* — that detail is
  gone. The source's own quality-vs-original is the achievable floor; clamp to it.
- **A few (source, dest) pairs share a coefficient representation** and skip pixels
  entirely (JPEG↔JPEG, JPEG→JXL, JXL→JPEG). Best cells: no generation loss.

---

## Target semantics — `QualityTarget` enum (decided)

The public target is an enum; callers pick per-call, default `Absolute`:

```rust
pub enum QualityTarget {
    /// zensim-A vs the ORIGINAL image. We only have the (lossy) source, so predict
    /// the source's quality-vs-original floor and clamp to it — never promise
    /// detail the source discarded. The default; one comparable number everywhere.
    Absolute(f32),
    /// zensim-A vs the DECODED SOURCE pixels — a cap on generation loss. No floor
    /// model; the number isn't comparable across sources of different quality.
    Relative(f32),
}
```

Each cell maps this onto its codec's native target: `Absolute` → the codec's
zensim-A picker with a source-floor clamp (e.g. `zenjxl QualityTarget::Inferred`,
`zenavif QualityTarget::Zensim`); `Relative` → score against the decoded-source
reference directly (`zenjxl QualityTarget::Relative`). Mirrors what
`zenjxl::jpeg_lossy` already exposes.

---

## Two method classes (best available per cell)

### 1. Coefficient-domain native transcode (no pixel round-trip)
Best quality (no re-decode/re-encode generation loss), usually smallest, fastest.
Only where the pair shares a DCT/coefficient representation:

| pair | API | notes |
|---|---|---|
| JPEG→JPEG | `zenjpeg::recompress` | re-quantize DCT coeffs; auto strategy; never regresses size. **wired** |
| JPEG→JXL | `zenjxl::jpeg_lossy::recompress_jpeg_lossy_target` | coeff coarsening + lossless floor; native target. **wired** |
| JXL→JPEG | `zenjxl_decoder::reconstruct_jpeg` | byte-exact *iff* the JXL is a JBRD transcode; else → one-shot picker. **fn exists, not yet routed** |

### 2. One-shot picker (the default for every lossy dest)
Decode source → `zenanalyze` features → the dest codec's learned picker predicts the
knob for the `QualityTarget` → **encode once**. The picker was trained to land
on-target, so there is no search. Optionally score the single output to *report* the
achieved zensim-A (never to re-encode).

| dest | one-shot predictor | feature-aware? | maturity |
|---|---|---|---|
| AVIF | `EncoderConfig::auto_tune(rgb,w,h,QualityTarget::Zensim(t),opts)` (feat `auto-tune`) | ✓ MLP | **production** — only shipped zen picker |
| JPEG | `zenjpeg` `encode/picker.rs` (`picker_zenjpeg_a_v3_f16.bin`) | ✓ MLP | baked; needs a transcode entry |
| WebP | `zenwebp` `encoder/picker/` (`zenwebp_picker_v0.1.bin`) | ✓ MLP | v0.1, early |
| JXL  | `calibrated_jxl_quality(generic_q)` only | ✗ global map | **gap**: no zensim-A / feature-aware picker — see below |

**JXL-dest gap.** JXL has no learned zensim-A picker yet — only a global
`generic_q → distance` calibration. The dominant JXL case (JPEG source) is covered by
the T1 coefficient path. For *other* sources → JXL we either (a) chain
`target_zq → generic_q → distance` through calibration as a stopgap one-shot, or
(b) train a JXL picker (the `zenpicker-train` effort). Flag it; don't pretend it's
feature-aware.

**Picker-runtime status (the real blocker).** The AVIF/JPEG/WebP pickers above are
*baked*, but their runtime — `zenpredict` (ZNPR) + `zenanalyze` (feature extractor)
— is mid-consolidation in `zenanalyze/zenpicker-train`: zenpredict v3 is
unpublished, the codec picker hooks are research-gated (`__picker-research`, off in
CI) and path-pinned to drifted worktrees, and per the `zenpicker-train` audit
*nothing is wired back into the codecs yet*. Pulling any of them into zencodecs
today drags those provisional, worktree-only deps into the dispatch layer. So the
picker cells are **architecturally dispatch-ready** (`transcode_via_picker` is the
seam) but stay stubbed until either (a) zenpredict/zenanalyze publish and each codec
exposes its picker behind a *stable* feature, or (b) a per-codec zensim-A→knob
**calibration** sweep gives an unblocked, non-ML one-shot. Do **not** wire
worktree-only ML deps into zencodecs to light a cell early.

---

## The grid (lossy dests; method = best available)

`★`=wired, `·`=needs wiring.

| source ↓ \ dest → | **JPEG** | **WebP** | **AVIF** | **JXL** |
|---|---|---|---|---|
| **JPEG**  | coeff recompress ★ | picker · | picker auto_tune · | coeff jpeg_lossy ★ |
| **PNG / BMP / PNM / TIFF** | picker · | picker · | picker auto_tune · | calib stopgap · |
| **WebP**  | picker · | picker · | picker auto_tune · | calib stopgap · |
| **AVIF**  | picker · | picker · | picker · | calib stopgap · |
| **JXL**   | coeff reconstruct(JBRD)→picker · | picker · | picker auto_tune · | passthrough/picker · |
| **HEIC**  | picker · | picker · | picker auto_tune · | calib stopgap · |
| **GIF** (animated) | first-frame · | animated WebP · | animated AVIF · | animated JXL · |
| **RAW / DNG** | picker · | picker · | picker auto_tune · | calib stopgap · |

**Non-lossy dest columns** (target is degenerate or a budget): **PNG /
WebP-lossless / JXL-modular** → best lossless, or a near-lossless budget;
**GIF** → ≤256-color palette via `zenquant` (target → palette size + dither).

---

## What makes a cell *well done* (cross-cutting)

1. **Quality ceiling/floor.** Clamp the achievable target to the source's inferred
   quality-vs-original; `target > floor` → ship the best honest encode (T1 lossless
   transcode where available) rather than chasing unreachable detail.
2. **No size regression.** Never emit something larger for equal/greater quality;
   the source bytes win if a re-encode can't beat them at target.
3. **Alpha.** RGBA source → no-alpha dest (JPEG) → matte/flatten with `opts.matte`.
4. **Animation.** GIF / animated-WebP → animated dest = per-frame; → still dest =
   first frame (documented), not an error.
5. **HDR / gain maps.** Carry/transcode per `opts.supplements`; score the SDR base.
6. **Grayscale.** Keep 1-channel sources grayscale; don't expand to RGB.
7. **Metadata + color.** Thread `opts.metadata_policy` + `ColorEmitPolicy` (shipped).

---

## Build order

1. **`QualityTarget` enum + one-shot router.** Replace the `target_zq: f32` arg and
   the 2-cell match with a dispatch: coefficient path if the pair has one, else
   `decode → features → dest picker → encode once`. The router is the substrate;
   each cell is "which predictor."
2. **Route T1 `JXL→JPEG`** — plug `reconstruct_jpeg_from_jxl` in (byte-exact for JBRD
   JXLs; else fall to the JPEG picker).
3. **Wire dest pickers, feature-aware first:** **AVIF** (`auto_tune`, production) →
   **JPEG** (`encode/picker`) → **WebP** (`encoder/picker`). Each is one `predict →
   encode` cell sharing the decode + feature extraction.
4. **JXL dest:** calibration stopgap one-shot now; note the picker gap for
   `zenpicker-train`.
5. **Cross-cutting helpers** (alpha matte, animation per-frame, grayscale, gain-map
   carry) that every cell calls.

Each step is independently shippable and leaves the grid strictly more complete.

---

## Provenance
- Pickers: zenavif `src/auto_tune.rs` (`QualityTarget::Zensim`, production, feat
  `auto-tune`), zenjpeg `src/encode/picker.rs` (`picker_zenjpeg_a_v3`), zenwebp
  `src/encoder/picker/` (`zenwebp_picker_v0.1`), zenjxl `calibrated_jxl_quality`.
- Coefficient paths: zenjpeg `src/recompress/`, zenjxl `src/jpeg_lossy.rs`,
  zenjxl-decoder `reconstruct_jpeg`.
- Centralized picker effort (codec-agnostic, zenjpeg-only so far):
  `zenanalyze/zenpicker-train`.
- Current router: `zencodecs/src/transcode.rs::transcode_to_quality`.
