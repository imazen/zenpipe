//! Output policy before effects: a required live schedule refuses a buffering
//! encoder and names the unmet requirement; finish-first with a disk spool
//! then succeeds. `cargo run -p zenpipe-io-check --example output_policy -- in.png out.png spool.bin`
//!
//! The `PolicySet` is the operation policy. The driver admits every offer
//! against its requirements before selection, and the refusal report names the
//! unmet ones. A custom callback goes inside
//! `BoundPolicy::new(requirements, resolver)`.
use std::num::NonZeroUsize;

use zencodec::encode::EncoderConfig;
use zencodec::operation::{OutputSchedule, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, NoPulse};
use zencodec_io::row_encode::{self, Options};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};

fn rows<'a>(
    codecs: &CodecSet,
    bytes: &'a [u8],
) -> Result<
    PipelineRows<DecodeSource<zencodec::source::SliceReadAt<'a>>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let source = DecodeSource::from_slice(
        codecs,
        bytes,
        DecodeOptions::new(StorageBudget::new(256 << 20, 64)),
    )?;
    Ok(PipelineRows::from_source(source)?)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output, spool] = &args[..] else {
        return Err("usage: output_policy IN OUT SPOOL".into());
    };
    let codecs = CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new());
    let bytes = std::fs::read(input)?;
    // Effort 19 buffers the image and emits at finish: no live route exists.
    let buffered = || {
        zenpng::PngEncoderConfig::new()
            .with_generic_effort(19)
            .job()
    };
    let live = PolicySet::new().require_output_schedule(OutputSchedule::Live);
    let refused = row_encode::encode_job(
        buffered(),
        rows(&codecs, &bytes)?,
        std::io::sink(),
        live,
        Options::new(StorageBudget::new(64 << 20, 1024), 256 << 20),
        &NoPulse,
    )
    .unwrap_err();
    println!(
        "refused at {:?} after {} bytes; unmet: {:?}",
        refused.error().stage,
        refused.error().emitted,
        refused
            .error()
            .report
            .decisions()
            .flat_map(|d| d.unmet_requirements.iter())
            .collect::<Vec<_>>()
    );
    let spool = std::fs::File::options()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(spool)?;
    let finish_first = PolicySet::new().require_output_schedule(OutputSchedule::FinishFirst);
    let options = Options::new(StorageBudget::new(64 << 20, 1024), 256 << 20)
        .with_disk_staging(spool, NonZeroUsize::new(64 * 1024).unwrap())?;
    let done = row_encode::encode_job(
        buffered(),
        rows(&codecs, &bytes)?,
        std::fs::File::create(output)?,
        finish_first,
        options,
        &NoPulse,
    )?;
    println!(
        "{} bytes via a disk spool ({:?})",
        done.emitted, done.schedule
    );
    Ok(())
}
