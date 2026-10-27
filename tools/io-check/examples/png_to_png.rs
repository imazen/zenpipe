//! Smallest route: owned input bytes -> controlled decode -> crop -> planned
//! PNG encode into memory. `cargo run -p zenpipe-io-check --example png_to_png -- in.png out.png`
use zencodec::encode::EncoderConfig;
use zencodec::operation::PolicySet;
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, NoPulse};
use zencodec_io::row_encode::{self, Options};
use zenpipe::Source;
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        return Err("usage: png_to_png IN OUT".into());
    };
    let codecs = CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new());
    let source = DecodeSource::from_bytes(
        &codecs,
        std::fs::read(input)?,
        DecodeOptions::new(StorageBudget::new(256 << 20, 64)),
    )?;
    let (w, h) = (source.width(), source.height());
    // Crop a centered half; operators need the owned (`from_bytes`) source.
    let crop = zenpipe::sources::CropSource::new(Box::new(source), w / 4, h / 4, w / 2, h / 2)?;
    let rows = PipelineRows::from_source(crop)?;
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    let mut out = Vec::new();
    let done = row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new(),
        Options::new(StorageBudget::new(64 << 20, 1024), 256 << 20),
        &NoPulse,
    )?;
    std::fs::write(output, &out)?;
    println!("{} bytes, schedule {:?}", done.emitted, done.schedule);
    Ok(())
}
