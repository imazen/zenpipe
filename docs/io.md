# Pipelines over zencodec-io: sources, planned encoding and output schedules

`zenpipe::io` (feature `io`; `io-tokio` adds the Tokio bridge) connects zenpipe's
pull-based `Source` graph to the zencodec 0.2 drivers. A controlled decoder reads
from a slice, a file or a range source; pipeline operators process its strips;
the planned row encoder plans the retained encoder job, resolves the output
schedule under policy, and only then reads the first row or writes the first
byte. Qualification lives in `tools/io-check` (unpublished; its tests author their
own samples and read PNG output back with the independent `png` crate).

## Slice in, PNG out

```rust
use zencodec::{CodecSet, NoPulse, encode::{EncodeJob, EncoderConfig}, operation::PolicySet,
               storage::StorageBudget};
use zencodec_io::row_encode::{self, Options};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};

let codecs = CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new());
let source = DecodeSource::from_slice(&codecs, &input, DecodeOptions::new(StorageBudget::new(64 << 20, 64)))?;
let rows = PipelineRows::from_source(source)?;
let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
let mut out = Vec::new();
row_encode::encode_job(job, rows, &mut out, PolicySet::new(),
                       Options::new(StorageBudget::new(16 << 20, 64), 16 << 20), &NoPulse)?;
```

Construction reads the input, answers every mandatory decode question through the
policy, and stops at the final interpretation without adopting it. Geometry,
format and `info()` (orientation, color carriers, EXIF/XMP) are final, but no pixel
row is decoded until the first pull. An encode refusal in between costs no pixel
decode.

## Processing: operators need owned input

zenpipe operators box a `'static` upstream (`CropSource::new(Box<dyn Source>, ..)`).
A decoder that borrows its input slice cannot feed them. Use `from_bytes` with an
owned buffer; the bytes are shared, not copied, and stay resident for the
source's lifetime:

```rust
let source = DecodeSource::from_bytes(&codecs, input_vec, options)?;
let crop = zenpipe::sources::CropSource::new(Box::new(source), 5, 3, 61, 47)?;
let rows = PipelineRows::from_source(crop)?;
```

`SharedBytes` removes a copy; it does not make memory bounded. The current JPEG and
PNG controlled routes retain their whole compressed input before their final
interpretation in any case.

## Declared color

The encoder writes its color carriers into the header, before it sees a row, so the
interpretation of the rows must be declared up front. `PipelineRows::from_source`
takes it from `Source::color_context()`: the decoder states its interpretation, and
operators that keep their upstream's (crop, 8-bit resize, horizontal flip, edge
replication, canvas expansion, tracing, orientation rendering) pass it on. Every
other source reports unknown by default, including any wrapper you write that does
not delegate, and `from_source` then refuses with `Refusal::UnknownColor` rather than
guess. After a conversion, declare the converted rows' context with
`PipelineRows::new(source, Some(context))`. A strip that carries a different context
than the declared one refuses.

## Files and range sources

Any `zencodec::source::ReadAt` works: `zencodec_io::FileReadAt`, an object-store or
HTTP adapter, a cached transport. `DriveOptions` set the sequential read size and
the gap over which independent ranges merge into one `read_many_at` round.
`encode_job_to_destination` with `zencodec_io::destination::SeekBack` lets encoders
that patch at finish use a seekable file; `Forward` covers sockets and pipes.

```rust
let file = zencodec_io::FileReadAt::open(&path)?;
let source = DecodeSource::new(&codecs, &DecodeRequest::new(), file, PolicySet::new(),
                               options.with_drive(DriveOptions::new(0, 64 * 1024)))?;
let destination = zencodec_io::destination::SeekBack::new(std::fs::File::create(&out_path)?)?;
row_encode::encode_job_to_destination(job, PipelineRows::from_source(source)?, destination,
                                      PolicySet::new(), encode_options, &NoPulse)?;
```

A sequential codec cannot skip bytes for a region: its transport cost is the number
of dependent rounds, `ceil(len / read_size)` reads that each wait for the previous
one. Over a high-latency link, read the whole object in one request (or stream one
GET) rather than many small ranges. Range selection pays off only for
random-access formats.

## Tokio

zencodec-io has no async planned encoder, so `io::tokio_bridge` runs the
synchronous pipeline on Tokio's blocking pool. `BlockingReadAt` serves reads from an
`AsyncSource` (batched ranges stay one `read_ranges` call), and
`drive_to_async_writer` drains a bounded channel into an `AsyncWrite` while the
pipeline runs:

```rust
let handle = tokio::runtime::Handle::current();
let (completed, delivered) = drive_to_async_writer(4, move |writer| {
    let reader = BlockingReadAt::new(async_source, handle);
    let source = DecodeSource::new(&codecs, &DecodeRequest::new(), reader, PolicySet::new(), options)?;
    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(row_encode::encode_job(
        job, PipelineRows::from_source(source)?, writer, policy, encode_options, &pulse)?)
}, &mut async_writer).await?;
```

One blocking-pool thread is held for the whole operation, including network waits.
Draining after the work completes deadlocks once the output exceeds the channel's
capacity, under either schedule; the bridge drains concurrently. Dropping the future
closes the channel, and the blocking work stops at its next write or pulse check.

## Output schedules are a policy choice, not a free speedup

The driver offers `FinishFirst` always and `Live` only when the configured encoder
streams (a PNG at effort 0 does; effort 19 buffers the image and emits at finish).

| | Live | FinishFirst |
|---|---|---|
| first byte | before the first source row (the header) | after the encoder finished |
| held while a write waits | the row buffer, the source and the encoder's native state | the staged compressed output (memory, or a disk spool with a bounded replay buffer) |
| source or writer failure | leaves a partial publication; `Failure::emitted` counts it | nothing published before the encoder finished |
| output limit | discovered while publishing | discovered before any write |

Slowing a streaming encoder bounds its queues. Slowing one that retains a whole
raster keeps that raster alive for a slow client; finishing into bounded staging
releases it sooner. `PolicySet::require_output_schedule` makes the choice mandatory.
Put requirements in the driver's preset, `Options::with_policy_set(policy)`, and pass
the same policy as the resolver: the driver admits every offer against the preset
before it asks the resolver, so an impossible requirement refuses before any row is
read or byte written, and the error's decision report names the unmet requirement.
The same `PolicySet` passed only as the resolver also refuses before any effect, but
the report names nothing (zencodec `79676215` evaluates requirements from the preset
alone). `ImageJob::with_output_policy` installs its policy in both places. A required
wait-memory bound (`with_output_wait_limit`) refuses today with
`(OutputWait, Unknown)`: the driver reports no qualified bound.

```rust
let policy = PolicySet::new().require_output_schedule(OutputSchedule::Live);
row_encode::encode_job(job, rows, writer, policy,
                       Options::new(budget, max_output).with_policy_set(policy), &pulse)?;
```

## Orientation

Decoders default to `OrientationHint::Preserve`: stored pixels plus the reported
orientation. Passthrough keeps both and carries the orientation in the output
metadata; nothing is held. `OrientationCost` reports what rendering upright costs a
single-pass pipeline: `Identity` and `FlipH` stream; `FlipV` and `Rotate180` reverse
rows; the four axis swaps read stored columns. All six hold the whole stored raster
before the first upright row. `render_orientation` admits that raster from a
`StorageBudget` before reading any row (so an insufficient budget refuses with no
pixel work), then serves upright strips from it. After rendering, set the metadata
orientation to `Identity`: the metadata filter rewrites the EXIF tag so a viewer
does not rotate again. `MetadataPolicy::ColorAndRotation` keeps the orientation and
drops private EXIF fields such as artist and copyright.

## Copies and admitted memory

Per decoded row: the native batch is copied into the controlled decoder's delivery
buffer, then into a held strip slot; operators read the slot in place; the encode
driver's row buffer receives one more copy. Strip slots, the driver's row buffer and
its compressed staging are admitted from `StorageBudget`s. Native codec workspace,
the controlled decoder's retained input, encoder-internal buffering and operator
buffers (resize rings, the carry buffer in `PipelineRows`) are not.

## Coverage and gaps

Qualified paths: controlled JPEG and PNG decode; planned PNG row encode; slice,
owned bytes, file, modeled range transport and strict loopback HTTP sources; Tokio
bridge on multi-thread and current-thread runtimes.

Explicitly untested: the zenpipe package's own test targets (its dev-dependency
zengif is not on the zencodec 0.2 Pulse contract yet); zencodecs `stop_and_limits`
(needs webp and gif). Region/range efficiency needs a random-access codec on the 0.2
graph (TIFF, JXL, IMI); none is in zenpipe's graph today, and the controlled decode
request carries no crop hint (only the legacy `DecodeJob::with_crop_hint` does), so a
crop is applied after a full decode.

Integration findings (2026-10-28):

- **Fixed in zenjpeg `3d2c8d5b`: JPEG controlled decode exceeded a 2 MiB thread
  stack in unoptimized builds.** At `af5fd093` the first pull started zenjpeg's
  streaming decoder inside the controlled coroutine; at the guard page the stack
  held 33 frames (no recursion) and 2,095,368 bytes, 2,022,064 of them in four
  zenjpeg frames (`scanline_reader_inner` 1,151,552, `streaming_decoder` 452,864,
  `scanline_reader_stopped` 249,872, the controlled start closure 159,264).
  zenpipe's adapter frames total about 7 KB. Spawned `std` threads and Tokio's
  blocking pool default to 2 MiB. `3d2c8d5b` boxes the Huffman table sets; the
  tests run on the default test thread, with no stack override.
  `just io-stack-frames <target> <test>` repeats the measurement.
- **Fixed in zenjpeg `8672e549` / zenpng `5422b6d2`: a direct codec job dropped an
  orientation that no EXIF payload carried.** Retention keeps the authoritative
  `Metadata::orientation` field; the JPEG and PNG encoders now write a field-only
  orientation as EXIF and refuse when EXIF is forbidden (zenjpeg `6c548022`,
  zenpng `1cb9976e`). zencodecs' `EncodeRequest` also folds a field-only
  orientation into a minimal EXIF payload for JPEG, PNG and WebP before the codec.
- **Fixed here (`ImageJob`): `WebDefault` and `StripAll` dropped the orientation
  without rendering it**, so a rotated photo displayed sideways on both routes.
  Every policy now carries the source orientation to the encoder unless a
  `zenlayout.orient` node with a concrete value renders it, in which case the
  output says `Identity`. Still open on this base (zenpipe issue 107): with an
  orient node the rendered pixels are not the exact rotation (5,715 of 18,915
  samples differ, by up to 95, on both routes), and `autorotate=true` (the
  `Orient` sentinel 0, not resolved when the geometry is compiled) fails in
  `ResizeSource` with a ring-buffer overflow that main has since fixed.
- **Fixed in zencodec `79676215`: one PNG native batch was delivered in one step.**
  zenpng returns 32 KiB of rows per batch (a 97x65 RGB image arrives whole); at
  `f5a7fd46` the coroutine decoder copied every sub-strip of a batch into the sink
  before returning, so the strip queue had to hold the whole batch (65 one-row
  slots against a 64-object budget). The coroutine now keeps the batch and lends
  one admitted strip per step; the same test passes with one slot and the budget
  unchanged.

Not solved here, and not implied by the still-image tests:

- **Late metadata.** The row driver fixes the description (geometry, descriptor,
  color) before the first row; a source whose interpretation changes later fails
  with `SourceChanged` rather than relabeling earlier rows. Metadata a decoder
  discovers after its final interpretation (trailing EXIF, a late ICC) is outside
  `DecodeSource::info()`.
- **Delayed frames.** These adapters move one image. Encoders that hold frames back
  (AV1 lookahead, B-frames) need a sequence driver with frame-level timing; the
  still-image `OutputTiming` says nothing about them.
- **Codec-owned allocations.** Native workspace, encoder-internal buffering and the
  controlled decoders' retained input are not admitted storage, so no
  whole-operation memory bound follows from these budgets.
