//! Positional file input, seekable file output, no resident copy of the input
//! made by zenpipe (the JPEG/PNG decoders still retain their compressed input).
//! `cargo run -p zenpipe-io-check --example file_to_file -- in.jpg out.png`
use zencodec::CodecSet;
use zencodec::encode::EncoderConfig;
use zencodec::operation::{DecodeRequest, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec_io::destination::SeekBack;
use zencodec_io::row_encode::{self, Options};
use zencodec_io::{DriveOptions, FileReadAt};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        return Err("usage: file_to_file IN OUT".into());
    };
    let codecs = CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new());
    let source = DecodeSource::new(
        &codecs,
        &DecodeRequest::new(),
        FileReadAt::open(input)?,
        PolicySet::new(),
        DecodeOptions::new(StorageBudget::new(256 << 20, 64))
            .with_drive(DriveOptions::new(0, 1 << 20)),
    )?;
    let stats = source.stats();
    let rows = PipelineRows::from_source(source)?;
    let destination = SeekBack::new(std::fs::File::create(output)?)?;
    let done = row_encode::encode_job_to_destination(
        zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
        rows,
        destination,
        PolicySet::new(),
        Options::new(StorageBudget::new(64 << 20, 1024), 256 << 20),
        None,
    )?;
    println!(
        "read {} bytes in {} reads before the first row; wrote {} bytes ({:?})",
        stats.drive.bytes_read, stats.drive.reads, done.emitted, done.schedule
    );
    Ok(())
}
