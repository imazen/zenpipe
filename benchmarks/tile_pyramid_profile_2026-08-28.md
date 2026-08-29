# Tile pyramid: memory + time profile (zenpipe#24)

**Date** 2026-08-28 · **Commits** `275c5dbb` (harness), `5b3c8101` + `2fe50306`
(the fixes this profile motivated) · **Issue** [#24](https://github.com/imazen/zenpipe/issues/24)

What `tiles::TilePyramidSink` / `PyramidWriter` actually cost, where the time
goes, and — the question that matters most — **which input formats can feed it
without defeating its bounded-memory property**. Short answer to the last one:
today only JPEG, PNG, WebP and GIF can. JXL and TIFF cannot, and the reason is
in their `zencodec` adapters, not in the pyramid.

---

## 1. Method

Two harnesses, both in-tree:

| harness | measures |
|---|---|
| `examples/tile_pyramid_mem.rs` | process max RSS only (the original #24 probe) |
| `examples/tile_pyramid_profile.rs` | **peak live heap**, **allocation count + bytes**, wall, across `--layout / --store / --threads / --encode / --source` |

`tile_pyramid_profile` installs a counting `GlobalAlloc` that forwards to
`System` and tracks `allocated − freed`. Three numbers come out of every run
and they are **not** interchangeable:

- **`peak_live_mb`** — high-water mark of live heap. Excludes the binary,
  stacks and allocator slack. This is the number to compare against
  `buffer_bytes_estimate()`.
- **`allocs` / `alloc_mb`** — total allocation calls and bytes requested, i.e.
  churn. Two runs with the same peak and a 10× `allocs` gap differ in malloc
  pressure, not footprint.
- **`max_rss_mb`** — `/usr/bin/time -l` (macOS) / `-v` (Linux). Always ≥ peak
  live heap.

`--source callback` generates rows on the fly, so the source holds no frame and
every measured byte belongs to the sink.

### Platforms

| tag | machine | tool |
|---|---|---|
| **M4** | Apple M4 Pro, macOS 26.5, 12 cores | `/usr/bin/time -l` + counting allocator + `sample` |
| **7950X** | x86_64 Linux, 32 cores / 60 GB (`dev`) | **`heaptrack`** + `/usr/bin/time -v` |

heaptrack is Linux-only; every number below is labelled with which platform and
which tool produced it. Nothing here is extrapolated.

### The counting allocator is trustworthy — heaptrack says so

Same binary, same cells, 7950X. `peak_live_mb` is the counting allocator,
`heaptrack peak` is `heaptrack_print`'s "peak heap memory consumption":

| cell | peak_live_mb | heaptrack peak | max RSS |
|---|---|---|---|
| 4096×4096 callback | 13.0 | 13.03 M | 16.8 |
| 8000×8000 callback | 25.0 | 25.02 M | 28.6 |
| 8000×8000 materialized | 280.4 | 280.51 M | 278.1 |
| 8000×8000 spool | 25.0 | 25.06 M | 28.7 |
| 40000×1000 callback | 115.3 | 115.35 M | 119.4 |
| 100000×600 callback | 279.8 | 279.87 M | 284.7 |

Agreement is within 0.1 % on every cell, so **the macOS counting-allocator
numbers in the rest of this document stand in for heaptrack** on a host where
heaptrack does not exist. The one row that legitimately differs is
`--source jpeg` (45.2 vs 62.17 M): the harness resets its counters *after*
synthesizing the JPEG, heaptrack counts the whole process.

---

## 2. Where the peak actually lives (heaptrack, 7950X)

100000×600 RGBA8 → DZI 254/1, JPEG tiles, 8 threads, `FsStore`.
Peak heap **289.17 MB**, total leaked 5.97 KB. heaptrack's peak-consumer
attribution:

| call site | peak | share | what it is |
|---|---|---|---|
| `TilePyramidSink::new` | **103.29 MB** | **35.7 %** | the tile-row scratch, one up-front allocation |
| `Sink::consume` | 102.40 MB | 35.4 % | level-0 row queue (600 calls — one per source row) |
| `push_row` | 67.70 MB | 23.4 % | row queues of levels ≥ 1 + shrink outputs |
| `__rust_alloc` (180 792 calls) | 8.94 MB | 3.1 % | JPEG encoder working set |
| `__rust_alloc_zeroed` | 6.63 MB | 2.3 % | — |

**The single biggest consumer is the tile-row scratch, and it is not a queue.**
`TilePyramidSink::new` allocates `cols × (tile + 2·overlap)² × bpp` up front so
`emit_tile_row` can hand the writer a whole tile row at once
(`394 × 256 × 256 × 4 = 103 284 736` bytes here). See §7 for what to do about it.

---

## 3. Size sweep — RSS is a function of *width*, not pixels

M4, DZI 254/1, RGBA8, `--store sink-only`, 1 thread, steady state of 5 runs.

| image | MP | levels | tiles | wall s | allocs | alloc MB | peak live MB | formula MB | max RSS MB |
|---|---|---|---|---|---|---|---|---|---|
| 256 × 256 | 0.07 | 9 | 12 | 0.000 | 818 | 0.6 | 0.9 | 1.0 | 2.9 |
| 1024 × 1024 | 1.0 | 11 | 46 | 0.001 | 3 152 | 9.8 | 3.3 | 3.4 | 5.4 |
| 4096 × 4096 | 16.8 | 13 | 416 | 0.014 | 12 434 | 156.7 | 13.0 | 12.9 | 15.3 |
| 8000 × 8000 | 64.0 | 14 | 1 373 | 0.052 | 24 209 | 597.5 | 25.0 | 24.8 | 28.0 |
| 10000 × 1000 | 10.0 | 15 | 229 | 0.009 | 3 092 | 93.4 | 29.0 | 31.1 | 39.0 |
| 40000 × 1000 | 40.0 | 17 | 879 | 0.042 | 3 100 | 373.4 | 115.3 | 123.7 | 130.0 |
| 100000 × 600 | 60.0 | 18 | 1 785 | 0.063 | 1 904 | 560.2 | 279.8 | 308.9 | 376.1 |

The design claim holds and is worth restating precisely: **a 64 MP square image
peaks at 25 MB while a 60 MP image that is 100 000 px wide peaks at 280 MB.**
Same pixel count, 11× the memory — because every buffer is
`Σ_levels w_level × (tile + 2·overlap) × bpp` and the height never enters.

`buffer_bytes_estimate()` is a good upper bound: it is within 8 % on every row
and never under-predicts by more than 2 MB.

**Tiny inputs carry a real fixed cost.** At 256×256 the sink's own buffers are
0.9 MB for a 0.26 MB image — 3.4× the frame — because the row queue is
`tile + 2·overlap + 1 = 257` rows deep at *every* level whether or not the level
is that tall. Below roughly 2 × tile_size in either dimension, tiling costs more
memory than just decoding the image.

### Tile size is the memory dial (40000 × 1000, M4)

| tile | tiles | wall s | peak live MB | max RSS MB |
|---|---|---|---|---|
| 128 | 3 378 | 0.043 | 61.7 | 72.1 |
| 254 (DZI) | 879 | 0.043 | 115.3 | 130.0 |
| 256 | 875 | 0.043 | 114.6 | 126.3 |
| 512 (IIIF) | 248 | 0.055 | 217.6 | 291.2 |
| 1024 | 91 | 0.045 | 384.3 | 395.4 |

Memory is **linear in tile size** (both the queues and the scratch scale with
it) while wall time is flat to within noise. Halving tile size from 254 to 128
halves the memory for free. If a deployment is memory-bound on a very wide
image, tile size is the first knob, not thread count.

---

## 4. Layouts (M4, each at its own tile convention, `--tile 0`)

Comparing layouts at a forced common tile size is misleading — DZI is 254/1,
IIIF 512/0, Zoomify and Google Maps 256/0 — so these use each layout's own.

**4096 × 4096** (a size that is exactly `256 × 2⁴`):

| layout | tile | levels | tiles | wall s | peak live MB |
|---|---|---|---|---|---|
| DZI | 254/1 | 13 | 416 | 0.016 | 13.2 |
| IIIF 3 | 512/0 | 13 | 94 | 0.015 | 25.9 |
| Zoomify | 256/0 | 5 | 341 | 0.015 | 12.9 |
| Google Maps | 256/0 | 5 | 341 | 0.015 | 12.9 |

**10000 × 1000** (no padding layouts):

| layout | levels | tiles | wall s | peak live MB |
|---|---|---|---|---|
| DZI | 15 | 229 | 0.010 | 29.2 |
| IIIF 3 | 15 | 70 | 0.011 | 55.5 |
| Zoomify | 7 | 221 | 0.010 | 28.9 |

Layout choice costs nothing in itself. IIIF's 2× memory is entirely its 512 px
tile convention (§3), not the layout.

### Google Maps padding is the one real trap

`PyramidGeometry::PaddedSquare` grows the canvas to `256 × 2^k`. When the image
is already such a square that is free; when it is not, everything scales with
the padded canvas. 5000 × 3000 (15 MP) pads to 8192² (67 MP):

| layout | tiles | wall s | allocs | alloc MB | peak live MB |
|---|---|---|---|---|---|
| Google Maps | **1 365** | **0.097** | 23 598 | 775.9 | 25.6 |
| DZI (same image) | 332 | 0.014 | 10 545 | 221.2 | 15.7 |

**4.1× the tiles and 6.9× the wall time**, and ~75 % of those tiles are pure
background. `PyramidWriter::with_skip_blanks` exists for exactly this and is
**not on by default** — for a Google Maps pyramid of a non-square image it
should be. Worth documenting at the `google_maps()` constructor.

---

## 5. Stores and threads (M4, 10000 × 1000 DZI, 229 tiles)

| store | encode | wall s | peak live MB |
|---|---|---|---|
| `sink-only` (no writer) | — | 0.009 | 29.0 |
| null store | raw | 0.010 | 29.2 |
| `MemoryStore` | raw | 0.011 | **74.8** |
| `FsStore` | raw | 0.026 | 29.2 |
| `ZipStore` | raw | 0.038 (**was 0.105**) | 29.2 |

`MemoryStore` holds every encoded tile — 2.6× the peak here, and it grows with
the image. It is for tests and small pyramids, as documented; do not reach for
it on anything gigapixel.

**`ZipStore` was 4.4× slower than `FsStore` and all of the gap was its
CRC-32.** The byte-at-a-time table loop ran over 60 MB of tile bytes at roughly
600 MB/s. Slicing-by-8 (`2fe50306`) took zip from 0.105 s to 0.038 s — its
overhead over `FsStore` fell 6.8× (81 ms → 12 ms). Fixed.

### Thread scaling — limited by the serial sink pass, not by the pool

`--encode jpeg` (real q80 4:2:0 tiles) → `FsStore`, M4, 12 cores:

| threads | 10000×1000 wall s | 4096×4096 wall s |
|---|---|---|
| 1 | 0.124 | 0.212 |
| 2 | 0.073 | — |
| 4 | 0.048 | 0.084 |
| 8 | 0.038 | — |
| 12 | 0.038 | 0.068 |

3.3× and 3.1× on 12 cores looks poor until you account for Amdahl. The sink's
own pass (row copy + 2×2 shrink + store writes) is serial and measures 0.026 s
of the 0.124 s single-threaded total. Ideal 12-thread time is therefore
`0.026 + 0.098/12 = 0.034 s`; measured 0.038 s is **84 % of the theoretical
bound**. The encode pool is fine; *the serial sink pass is the ceiling*, which
is why §7 makes the sink pass itself faster rather than adding threads.

Peak memory grows only 29.2 → 32.0 MB from 1 to 12 threads (one encoder working
set per thread) — parallel tile encode is close to free memory-wise.

### End to end, gigapixel width

100000 × 600 → DZI, JPEG tiles, 8 threads, `FsStore`: **0.251 s, 1 785 tiles,
289.2 MB peak live / 325.1 MB RSS** (M4). heaptrack on 7950X: 289.17 MB peak,
5.97 KB leaked.

---

## 6. Where the time goes (`sample`, M4, 10 s windows)

**Sink only**, 8000×8000, before the §7 fixes — 7 620 samples:

| symbol | samples | share |
|---|---|---|
| `tiles::shrink_rows` | 4 721 | **62 %** |
| `_platform_memmove` | 1 699 | 22 % |
| (harness row generator) | 603 | 8 % |
| `__bzero` | 232 | 3 % |
| malloc/free | ~100 | 1.3 % |

**With a real JPEG encoder + `FsStore`**, 8000×8000, 9 222 samples:

| group | samples | share |
|---|---|---|
| zenjpeg + zenyuv encode | ≈ 6 800 | **74 %** |
| — of which `huffman::generate_code_lengths` | 902 | 9.8 % |
| — `entropy::encode_block_to_writer` | 1 525 | 16.5 % |
| — `zenyuv::rgb_to_yuv420_with` | 1 368 | 14.8 % |
| filesystem syscalls (`open`/`write`/`close`) | ≈ 620 | 6.7 % |
| `tiles::shrink_rows` | 541 | 5.9 % |
| `_platform_memmove` | 355 | 3.9 % |

Two things to take from this:

1. **Once a real codec is attached, the pyramid is ~10 % of the work and the
   encoder is ~75 %.** Optimizing the sink past this point has a small ceiling;
   the leverage is in per-tile encoder cost.
2. **`generate_code_lengths` alone is 9.8 %** — Huffman table construction paid
   once per *tile*, 1 373 times, with `optimize_huffman(false)`. This is the
   classic tiny-image fixed-cost problem: a 256×256 tile pays a whole JPEG
   image's fixed overhead. A shared/pre-built Huffman table across a pyramid's
   tiles is a codec-side change (zenjpeg), not a zenpipe one, but it is the
   single largest identified saving in the whole pipeline. Filed below.

---

## 7. What was fixed, and what it bought

### Fixed in `5b3c8101` — the per-even-row copy

`Level::pending` held a `row.clone()` of every even row so it could pair with
the next row one level down: one full-width allocation + memcpy per even row per
level. `emit_ready_tile_rows` now keeps the newest row instead of dropping it
(`keep_from.min(next_row - 1)`), so `push_row` reads the pair straight out of
the queue. The retained row was already covered by the `+1` in
`buffer_bytes_estimate`.

**Allocations −25 %, allocated bytes −22 %** (8000×8000: 768.1 → 597.5 MB of
churn). Peak live is unchanged — the clone was transient.

### Fixed in `5b3c8101` — the alpha divide on opaque rows

`shrink_rows` was 62 % of the sink's time, most of it **three integer divides
per output pixel** in the alpha-weighted average. Fully opaque row pairs now
take the plain path, which is *bit-identical* there — with every alpha 255,
`a_sum` is 1020 and `(255·S + 510)/1020` reduces exactly to `(S + 2)/4` — after
one linear scan of the two rows. The remaining paths index fixed-size arrays
(`as_chunks::<N>`) instead of runtime-length slices.

`shrink_rows` dropped from **62 % to 54 %** of the sink's own pass — roughly
40 % less time per run once the sample counts are normalized by the matched
wall times below.

`shrink_rows_matches_reference_bit_for_bit` pins the result against the previous
per-pixel implementation across bpp 1–4 × alpha on/off × opaque/non-opaque ×
widths 1–33 and 1024. Output is unchanged, not merely close.

### Fixed in `2fe50306` — ZipStore CRC-32

Slicing-by-8 (see §5). `crc32_matches_byte_at_a_time_at_every_length` pins it
against the textbook form at every length 0..=300.

### Net effect on the sink's own pass

M4, `--store sink-only`, **both binaries built from the same tree and run
back to back, `--repeat 25`, last run reported.** Pre-fix binary is
`src/tiles.rs` as of `275c5dbb`.

| image | before | after | Δ |
|---|---|---|---|
| 10000 × 1000 | 0.013 s | 0.010 s | **−23 %** |
| 40000 × 1000 | 0.060 s | 0.046 s | **−23 %** |
| 4096 × 4096 | 0.021 s | 0.015 s | **−29 %** |
| 8000 × 8000 | 0.080 s | 0.057 s | **−29 %** |
| 100000 × 600 | 0.088 s | 0.066 s | **−25 %** |

> **Correction.** The commit messages of `5b3c8101` / `2fe50306` quote
> "26–33 %" and a `100000 × 600` figure of `0.092 → 0.064 s`. Those paired a
> *single-shot* pre-fix run against a *warmed* post-fix run and are
> optimistic. The matched back-to-back numbers above — **23–29 %** — are the
> ones to use. Commit messages are immutable; this table is the record.

### Measured and *rejected*: a recycled-row pool

A per-level free list of popped row buffers was implemented and measured. It
does not pay: rows are freed in bursts of `tile_size` at tile-row boundaries but
consumed one at a time, so a 2-row pool served **0.8 %** of rows (allocations
3 092 → 3 094, wall unchanged within noise), and a pool deep enough to absorb a
burst costs `tile_size` extra rows per level — exactly the memory the sink
exists to save. Reverted rather than left as dead complexity in a hot path.

---

## 8. Honest answer per input class

This is the part that decides whether the tile pyramid is useful today, and the
answer is not symmetric across formats.

`job.rs::decode_source` tries `build_streaming_decoder()` and, on any error,
**falls back to `decode_full_frame()` + `MaterializedSource`** — the whole frame
in RAM. So the question per format is simply: does its `zencodec` adapter
implement `streaming_decoder`?

Measured cost of each source class (M4 counting allocator; 8000×8000 also
cross-checked with heaptrack on 7950X):

| source | 4096×4096 peak / RSS | 8000×8000 peak / RSS | 40000×1000 peak / RSS |
|---|---|---|---|
| `callback` (ideal stream) | 13.0 / 15.0 | 25.0 / 27.7 | 115.3 / 125.4 |
| `jpeg` (real streaming codec) | 18.7 / 32.1 | 45.2 / 79.8 | 134.5 / 178.0 |
| `materialized` (full-frame decode) | **79.8 / 81.8** | **280.4 / 283.2** | **272.7 / 330.1** |
| `spool` (`TempFileSource`) | 13.0 / 15.2 | 25.0 / 28.4 | 115.4 / 128.0 |

### (a) JPEG XL — **no group-aware path exists, and the pipeline spools nothing**

Read against `zenjxl-decoder` main (`f1faec70`) and `zenjxl` main (`9226d3a3`):

- `zenjxl`'s `zencodec` adapter returns
  `Err(UnsupportedOperation::RowLevelDecode)` from `streaming_decoder`
  (`zenjxl/src/codec.rs:2463`), with a test pinning that
  (`streaming_decoder_unsupported`). Its `push_decoder` is
  `zencodec::helpers::copy_decode_to_sink` — a **full decode followed by a row
  copy**, not incremental decode.
- `zenjxl-decoder`'s public API has **no region, rect, ROI, crop or group
  accessor at all**. `JxlDecoderOptions` has no such field; `JxlDecoder`'s
  surface is `process` / `flush_pixels(buffers)` over whole-image output
  buffers. Group parallelism exists (`parallel` / the `threads` feature,
  rayon over groups) but is an *internal scheduling* detail with no address
  exposed to callers.

So the honest statement: **JXL's group structure is real and is exploited for
throughput inside the decoder, but nothing about it is reachable from zenpipe.**
There is no group-aware path to A/B against, so no such measurement was made —
the two things that could be measured are "full decode then tile" (the
`materialized` row above: **280.4 MB at 64 MP**) and "streaming" (25.0 MB), and
JXL is unavoidably the former.

`TempFileSource` cannot help here either. It spools *decoded rows*, so it can
only be built from a source that already exists; for JXL the frame has already
been materialized by the time the spool could be written. The spool solves
"replay without re-decoding", not "decode without materializing".

What would actually fix it, in dependency order: (1) `zenjxl-decoder` exposes a
per-group or per-row-band flush (the `flush_pixels` machinery is already
incremental internally — this is an API surface question); (2) `zenjxl`'s
adapter implements `streaming_decoder` on top of it; (3) zenpipe needs no change
at all — `job.rs` already prefers the streaming path. **Step 1 is the blocker
and it is not in this repo.**

### (b) TIFF — `TiledContainerAccess` does not exist; strips are not lazy

Read against `zentiff` main in `imazen/zenextras`:

- `TiffDecodeJob::streaming_decoder` returns
  `Err(UnsupportedOperation::RowLevelDecode)` (`zentiff/src/codec.rs:735`), with
  `streaming_decode_rejected` pinning it. `push_decoder` is again
  `copy_decode_to_sink`.
- `decode(data, config, cancel)` runs `image-tiff`'s whole-image read inside one
  `catch_unwind` and returns a complete `PixelBuffer`. There is **no page index
  parameter, no strip or tile accessor, and no `TiledContainerAccess` type** —
  confirming the gap named for #24 step 5, on main, today.
- `TiffDecodeConfig` exposes only limits (`max_pixels`, `max_memory_bytes`,
  `max_width/height`, `alloc_pref`) — nothing geometric.

So a 100 000 px-wide TIFF **cannot** be tiled without a full decode, and the
`materialized` row is what it costs. This is the format where the loss hurts
most: tiled and striped TIFF is *the* gigapixel interchange format, its strip
offsets are exactly the random-access index a tile pyramid wants, and
`image-tiff` can read individual strips — the capability is present in the
dependency and simply not surfaced through `zentiff`'s API.

### (c) Paginated / multi-page — orthogonal, and cheaply so

- `zentiff`'s `TiffInfo::page_count` is probed (it walks the IFD chain via
  `count_pages`), so multi-page TIFF is *detectable*; but `decode()` has no page
  selector, so only the first IFD is reachable through the current API.
- PDF (`zenpdf`) and animation frames are per-page/per-frame renderers by
  construction.

Per-page tiling is **orthogonal to the pyramid, not a benefit of it**: each page
is an independent image and gets its own independent pyramid. There is nothing
to share between pages — no row queue, no level cascade, no scratch, because the
pyramid state is entirely intra-image. The only thing paging buys is that peak
memory is `max over pages`, not the sum, which is true for free as long as one
page is decoded at a time. The pyramid needs no changes for paginated input; the
*decoders* need page selectors.

The one genuine interaction: for a paginated source, the `row_scratch` and the
level queues are sized from the **widest page** if the sink is reused. Building
a fresh `TilePyramidSink` per page is correct and costs one allocation.

### (d) Plain JPEG / PNG / WebP / GIF — this is where it works, and what the spool buys

These decode row-by-row, so `DecoderSource` streams and the sink's bounded-memory
property is preserved end to end. Measured with a real `zenjpeg` streaming
decode at 64 MP: **45.2 MB peak / 79.8 MB RSS**, against 280.4 / 283.2 MB for
the same image through a full-frame decode — **6.2× less peak heap**.

The JPEG row is above the ideal `callback` row (25.0 MB) by the decoder's own
working set, and its RSS (79.8) runs well above its live heap (45.2) — that gap
is allocator slack from the decoder's allocation pattern, not sink buffers.

**What the spool buys.** `TempFileSource` measured *identical* to the ideal
streaming source — 25.0 MB peak / 28.4 MB RSS at 64 MP, versus 280.4 / 283.2 for
holding the frame. It converts a full-frame RAM cost into a full-frame *disk*
cost plus one strip of RAM. So it is worth it exactly when:

- a second pass is needed over decoded rows (an analysis barrier: autodeskew,
  whitespace crop, statistics) **and** the frame does not fit comfortably; or
- the decode is expensive and non-repeatable.

It is **not** worth it when the source can simply be decoded again — for a
streaming codec, re-decoding a 64 MP JPEG costs ~0.27 s of CPU against a 256 MB
spool file write plus read-back. For those formats a second `DecoderSource` over
the same bytes is cheaper than the spool. The spool's home is the
already-materialized case, where it turns 280 MB of RAM into 25 MB.

---

## 9. Not fixed — filed with measurements

Ranked by measured size of the prize.

1. **The tile-row scratch is 35.7 % of peak and only threads need it.**
   `TilePyramidSink::new` allocates `cols × (tile+2·overlap)² × bpp` (103.29 MB
   at 100000×600, heaptrack-attributed) so `emit_tile_row` can hand
   `write_tile_row` a whole row of tiles at once. A **sequential**
   `PyramidWriter` (`threads <= 1`, the default) consumes those tiles one at a
   time and never needs more than one tile live — 256 KB instead of 103 MB, a
   **37 % peak reduction on the widest images**.
   *Proposal (public API — not made, needs approval):* add a defaulted
   `TileWriter::wants_tile_rows(&self) -> bool { true }`; `PyramidWriter`
   returns `self.threads > 1`. When false, `TilePyramidSink::new` sizes the
   scratch to one tile and `emit_tile_row` emits per tile via `write_tile`.
   Additive, default-compatible, no behavior change for existing writers.

2. **Per-tile JPEG fixed cost — `generate_code_lengths` is 9.8 % of end-to-end
   time.** 1 373 tiles each build their own Huffman tables even with
   `optimize_huffman(false)`. A pyramid encodes thousands of small images with
   near-identical statistics; a table computed once and reused would remove most
   of that. Needs a zenjpeg-side API to supply pre-built tables (a codec-side
   change; zenpipe would just pass it through the `TileEncoder` closure).

3. **`with_skip_blanks` should be the default for `GoogleMapsLayout`, or at
   least documented at `TilePyramidConfig::google_maps`.** Measured: 5000×3000
   through Google Maps produces 1 365 tiles in 0.097 s versus DZI's 332 in
   0.014 s, ~75 % of them background (§4).

4. **`shrink_rows` is still 54 % of the sink's own pass** after the fixes. The
   plain path is a scalar per-channel `(sum + 2) / 4` over fixed-size arrays;
   it is a textbook `magetypes` kernel (u8 widen → add → shift → narrow) and
   should vectorize several-fold. Deferred because zenpipe's core is
   `no_std + alloc` with no archmage dependency today — adding one is a
   dependency decision, not a perf decision.

5. **No streaming decode for JXL / TIFF / AVIF / HEIC / RAW** (§8a, §8b). Owned
   by `zenjxl-decoder` + `zenjxl` and `zentiff` respectively. Until then, every
   tile pyramid built from those formats pays a full decoded frame — measured at
   **280.4 MB for a 64 MP RGBA8 image** versus 25.0 MB streaming.

6. **Tiny inputs cost more to tile than to decode** (§3). Worth a documented
   floor: below ~`2 × tile_size` in both dimensions the pyramid's row queues
   exceed the frame itself.

---

## 10. Reproducing

```bash
cargo build --release --example tile_pyramid_profile
target/release/examples/tile_pyramid_profile --tsv-header

# one cell
target/release/examples/tile_pyramid_profile \
    --width 100000 --height 600 --tile 254 \
    --layout dzi --store fs --encode jpeg --threads 8 --source callback

# max RSS around it
/usr/bin/time -l target/release/examples/tile_pyramid_profile ...   # macOS
/usr/bin/time -v target/release/examples/tile_pyramid_profile ...   # Linux
heaptrack   target/release/examples/tile_pyramid_profile ...        # Linux

# time profile: --repeat holds the process open for a sampler
target/release/examples/tile_pyramid_profile --width 8000 --height 8000 \
    --store sink-only --repeat 300 &
sample $! 10 -f /tmp/tiles.txt        # macOS
```

`--tile 0` uses each layout's own convention (DZI 254/1, IIIF 512/0, Zoomify
and Google Maps 256/0) — the only apples-to-apples way to compare layouts.

Raw grids: `benchmarks/tile_pyramid_profile_2026-08-28.tsv`.
