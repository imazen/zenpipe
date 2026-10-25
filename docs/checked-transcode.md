# Checked pixel transcoding

This implementation is behind `zencodecs/checked-transcode` on the review branch.
It uses zencodec's stored requests and one fallible policy resolver, with
zenpixels-convert's retained `OutputPlan`. It is not a replacement for every
existing transcode or sequence operation.

## One primary image

Register the codecs once, including the destination decoder used to inspect the
finished file. The ordinary call selects its prepared route automatically:

```rust
let output = TranscodeRequest::new(ImageFormat::Png)
    .with_cms(&MoxCms)
    .transcode(&codecs, input)?;
```

Defaults preserve current color, reported EXIF/XMP and rendering orientation.
Reported physical resolution must also survive; the shared encoder job cannot
yet carry it directly, so a destination that loses it refuses at verification.
They do not preserve unreported container packets. The native source must expose
controlled interpretation: this implementation qualifies the whole-source JPEG
route from zenjpeg PR 240 and PNG route from zenpng PR 46, with PNG and JPEG destinations. Other source codecs
refuse if they have no controlled factory. Sequences, auxiliary content, unknown
gain-map discovery, HDR rendering and unqualified float output refuse explicitly.

## An upload intended for an sRGB web image

Keeping an arbitrary source ICC may also keep profile descriptions or private
vendor fields. Dropping it without converting samples can change the displayed
colors. Request the conversion and descriptive-metadata treatment together:

```rust
use zencodec::{CodecSet, ImageFormat, operation::{MetadataTransfer, PolicySet}};
use zencodecs::checked_transcode::TranscodeRequest;
use zenpixels::{Cicp, ConvertOptions, PixelFormat};
use zenpixels_convert::{OutputProfile, cms_moxcms::MoxCms};

let codecs = CodecSet::new()
    .with_decoder(zenjpeg::JpegDecoderConfig::new())
    .with_decoder(zenpng::PngDecoderConfig::new())
    .with_send_encoder(zenpng::PngEncoderConfig::new());
let request = TranscodeRequest::new(ImageFormat::Png)
    .with_cms(&MoxCms)
    .with_output(OutputProfile::Named(Cicp::SRGB), PixelFormat::Rgb8,
                 ConvertOptions::forbid_lossy())
    .with_metadata(MetadataTransfer::DiscardDescriptive)
    .with_policy_set(PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false));
let output = request.transcode(&codecs, input)?;
```

RGB8 is explicit here because this example accepts JPEG. The conversion policy
refuses implicit alpha removal, gray conversion or depth reduction. An alpha
output cannot select a destination without native alpha support. Output checks
also require the selected integer sample depth. Orientation needed to render the
image is retained or reconciled even when source descriptions are discarded.
This is a contract about the reported metadata and selected carriers, not a
certificate that the image's pixels contain no PII.

`zencodecs/examples/checked_transcode.rs` is the compiled disk-input version.

## A tenant-specific decision

Use `transcode_with`, or `transcode_from_read_at_with`, when a preset is
insufficient. `Question::Transcode` contains complete prepared routes: confirmed
encoder configuration, converted sample descriptor, metadata treatment,
opaque-profile retention and scoped storage facts. A closure returns
`Result<Decision, At<ApplicationError>>`. It can choose an offered token, delegate
to the configured `PolicySet`, or return a predefined traced decline. The adapter
holds the actual conversion and encoder jobs while it calls the resolver; it
does not reconstruct a potentially different route from the chosen enums.

An opaque ICC is never parsed as XML to claim privacy. An application can require
a known output profile and discard descriptions, retain the opaque carrier under
an explicit policy, or refuse. Malformed XMP is preserved byte-for-byte by
`PreserveReported` and excluded by `DiscardDescriptive`; neither path attempts a
best-effort rewrite that could report a false guarantee.

## Disk and network sources

`transcode_from_read_at` uses the existing `zencodec::source::ReadAt` and
`zencodec_io::ReadAtCursor`. `FileReadAt` provides disk input; object-store/HTTP
adapters can retain batched range reads in their source implementation. Driver
admission and read sizes are set with `with_io_options(DriveOptions)`.
No second seek hierarchy is required. The `checked-transcode-async` feature adds `transcode_async` and
`transcode_async_with` over the existing `zencodec_io::tokio_io::AsyncSource`.
Register encoders with `CodecSet::with_send_encoder` to retain their concrete
job's Send guarantee across I/O waits. A legacy registration refuses before
reading input; it remains usable for synchronous requests.
Both use the same execution state, policy choices and output checks as the
synchronous calls. I/O waits do not repeat policy decisions. Dropping a pending
future releases its admitted source and row owners.

Async source reads run on the polling task; native decode, conversion and encode
steps also run there. `DriveOptions::work_quota` yields between whole steps and
cannot preempt a codec step. A CPU-heavy configuration needs a suitable worker
runtime. The default-policy future is Send when its source is Send. This is
async byte demand, not proof of small native working memory or short CPU steps.

The current JPEG and PNG controlled decoders retain the complete compressed source
before its final interpretation decision. Small driver read sizes do not make
that native implementation incremental. Source failures preserve their native
error object through `Error::source()`.

## Storage, output and failures

The default managed allowance is 64 MiB / 128 objects. It covers the controlled
JPEG/PNG source and adapter row buffers. Conversion/native workspace and encoded
allocation are not qualified bounds. `TranscodeOffer::workspace_upper_bound`
therefore remains `None`, and a preset requiring a known workspace bound refuses.
`admitted_bytes` describes the row/conversion storage retained after selection;
alternative fidelity choices share it and must not be summed.

The adapter never awaits an output writer while the encoder retains native
working state. It finishes the codec, releases rows and native state, inspects the
completed file, then returns output. `with_output_limit` limits publication; it
cannot prevent a native codec from allocating a larger encoded buffer first.
A future qualified sequential route needs its own ownership and output-pressure
evidence before it may expose incremental publication.

The postcondition checks cover geometry, orientation, integer depth, semantic
alpha, exact reported EXIF/XMP, authoritative output color and HDR/absolute-light
metadata. Native codec errors, callback errors and source errors retain their
original types and traces. The adapter error exposes a stage and the bounded
decision report; its default formatting excludes payloads and profile contents.
No failed execution retries another route.

## Qualification

`just checked-transcode-check` uses real baseline/progressive JPEG, an Adobe RGB
ICC, EXIF and malformed XMP. Converted PNG samples match a separately constructed
CMS transform and the independent png decoder. Profile-preserving JPEG retains
reported packets. A real encoder deliberately wrapped to discard descriptions
cannot publish success. File/range input agrees with the slice route; source
errors, cancellation, callback errors and panic cleanup remain observable.

The shared core separately tests stale choice identities, provisional facts,
bounded alternatives and unknown workspace rejection. `just
checked-transcode-compat` retains the existing adapter feature checks and public
API snapshots. These tests do not establish a native whole-operation RAM bound,
a throughput advantage, video epoch behavior or gigapixel viewport support.

### Pinned integration validation (2026-10-25)

Validated outside the workspace's sibling overrides with zencodec `14726d91`,
zenjpeg `686b0e27`, zenpixels `f8596aae` and zenpng `2802f8ee`. The native
checks and metadata-scrub suite pass, including independent PNG/CMS inspection,
exact retained JPEG carriers, callback unwind, source errors and the DPI
false-success regression. The core-only feature build, existing gain-map
adapters, metadata-service tool and both standalone fuzz-target builds pass.
The API snapshots include the new feature.

```text
just checked-transcode-ci
run-heavy: done rc=0 21s | peak-RSS 0.92GiB | min-avail 16097MiB | peak-load 1.49
just checked-transcode-locks checked-transcode-integrations
run-heavy: done rc=0 33s | peak-RSS 0.78GiB | min-avail 16289MiB | peak-load 2.16
```

These receipts describe validation processes, not codec memory bounds or speed.
The filter dependency migration also passed its library/integration correctness
tests and all-target Clippy. Its all-target test invocation entered development
benchmarks after the tests; that benchmark phase was interrupted, and supplies
no performance result. No existing test assertions were relaxed.

Async validation at zencodec `b28a659e` and zenjpeg `af8eee81` uses delayed
one-byte/7-byte/97-byte sources with both known and unknown length. Each produces
the same bytes as the verified synchronous operation and chooses its route once.
It also checks native source traces, callback refusal, cancellation, dropped
pending futures, zero-read admission refusal and transferable registration. A
compile-time Send assertion covers the complete default-policy future. The
root and four standalone dependency graphs resolve the coordinated revisions.

```text
just checked-transcode-locks
just checked-transcode-async-check
just api-doc
run-heavy: done rc=0 34s | peak-RSS 0.73GiB | min-avail 16926MiB | peak-load 0.90
```

## Prepared color and decoded precision (2026-10-25)

Adapter defaults now ask the checked encoder to emit the prepared color carriers
verbatim. Explicit request fields, or inherited registry fields, take priority.
Unsupported required fields fail during preparation, before source input.
A supplied Display P3 ICC consequently stays authoritative when transcoding
JPEG to PNG, instead of adding a derived cICP tag.

Output precision is checked against the inspection decoder's native output
descriptor, with no requested upconversion. PNG palette indices can use fewer
than eight bits while still representing exact eight-bit RGB values. Comparing
index width against channel precision falsely refused that lossless case. The
regression exercises actual packed PNG output and compares all expanded samples
with the decoded JPEG using both native and independent PNG readers. It does
not claim content-dependent fidelity from a descriptor alone.

Validation: native sync/async routes, required-policy precedence, metadata
violations, source/callback errors, cancellation and owner cleanup passed with
strict library/example/test Clippy (`just checked-transcode-async-check`). The
API snapshot gate also passed. This adds a second inspection query on completed
output; its parsing cost is not yet measured.

## Native PNG source qualification (2026-10-25)

`checked-transcode` enables the registered PNG decoder's controlled factory.
Lossless PNG-to-PNG checks cover RGB8, RGBA8, RGB16, RGBA16, Gray16 and
GrayAlpha16, with strided authored input, retained alpha/sample depth and exact
independent PNG readback. RGB ICC preservation and P3-to-sRGB conversion use
the same prepared route as JPEG; conversion matches a separate CMS transform.
Slice and positional sources agree. Delayed network sources with known or
unknown length, one-byte and 31-byte reads agree with synchronous output and
release managed storage after completion.

This qualification required native fixes: gain-map absence is established only
for a complete PNG with known chunks and no trailer, and untagged GrayAlpha16
uses the same sRGB assumption as the other integer layouts. Android/proposed
gain-map carriers, unknown extensions, APNG and unsupported color carriers
refuse before reconstruction. It does not qualify arbitrary PNG extensions,
native byte-resuming inflation or a complete operation memory bound.

Pinned zenpng `f6ac2539`, zencodec `78fe667c`, zenjpeg `c9f82fc0` and
zenpixels `f8596aae`; all five lock graphs resolve those sources. Native
integration, strict Clippy and the public API snapshot pass:

```text
just checked-color-integration
run-heavy: done rc=0 21s | peak-RSS 0.53GiB | min-avail 17073MiB | peak-load 0.49
```

This is a validation-process receipt, not a codec workspace or speed bound.

### Native batches and admitted strips

The sink now advertises the configured `with_strip_rows` capacity. The shared
coroutine delivery path divides larger native batches before borrowing buffers;
it does not grow the admitted rows. Native RGB8 and RGBA16 PNGs at 97x65 and
97x257 retain exact samples with 1-, 3- and 64-row destination strips. This
fixes the larger guide fixture's `StripGeometry` failure. Native batch and codec
workspace sizes remain separate from the adapter's strip allowance.

The complete native integration and strict Clippy/API checks pass with core
`939d5628`, PNG `3046bd77` and JPEG `02fa3866` (`just checked-color-integration`,
rc=0, 34 s; run-heavy peak RSS 0.72 GiB, minimum available 15496 MiB, peak
load 1.02). The root and four standalone dependency graphs agree.
