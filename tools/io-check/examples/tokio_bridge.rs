//! A Tokio task decodes async bytes and streams the planned encode to an async
//! writer through the blocking pool. `cargo run -p zenpipe-io-check --example tokio_bridge -- in.png out.png`
use zencodec::CodecSet;
use zencodec::encode::EncoderConfig;
use zencodec::operation::{DecodeRequest, OutputSchedule, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec_io::row_encode::{self, Options};
use zencodec_io::tokio_io::BytesSource;
use zenpipe::io::tokio_bridge::{BlockingReadAt, drive_to_async_writer};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        return Err("usage: tokio_bridge IN OUT".into());
    };
    let source = BytesSource(tokio::fs::read(input).await?.into());
    let handle = tokio::runtime::Handle::current();
    let mut file = tokio::fs::File::create(output).await?;
    let (done, delivered) = drive_to_async_writer(
        4,
        move |writer| -> Result<_, Error> {
            let codecs = CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new());
            let source = DecodeSource::new(
                &codecs,
                &DecodeRequest::new(),
                BlockingReadAt::new(source, handle),
                PolicySet::new(),
                DecodeOptions::new(StorageBudget::new(256 << 20, 64)),
            )?;
            Ok(row_encode::encode_job(
                zenpng::PngEncoderConfig::new().with_generic_effort(0).job(),
                PipelineRows::from_source(source)?,
                writer,
                PolicySet::new().require_output_schedule(OutputSchedule::Live),
                Options::new(StorageBudget::new(64 << 20, 1024), 256 << 20),
                None,
            )?)
        },
        &mut file,
    )
    .await
    .map_err(|e| -> Error { e.to_string().into() })?;
    println!(
        "{} bytes in {} chunks ({:?})",
        delivered.bytes, delivered.chunks, done.schedule
    );
    Ok(())
}
