//! One cell of the schedule/staging memory matrix, for heaptrack.
//!
//! `memory_probe make FIXTURE.png W H` writes a deterministic noisy RGB8 PNG.
//! `memory_probe run FIXTURE.png OUT.png EFFORT live|memory|disk|oneshot [SPOOL]`
//! decodes the fixture from a file, re-encodes it as PNG at EFFORT through the
//! planned row driver with the given schedule, and prints what the adapters held.
//! `oneshot` is the conventional baseline: read the whole file, decode the whole
//! raster, encode it whole. Run each cell in its own process under heaptrack; the
//! peak is whole-process heap, not RSS and not a bound for other sizes or
//! configurations.
use std::num::NonZeroUsize;

use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{DecodeRequest, OutputSchedule, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, NoPulse};
use zencodec_io::destination::Forward;
use zencodec_io::row_encode::{self, Options};
use zencodec_io::{DriveOptions, FileReadAt};
use zenpipe::io::{DecodeOptions, DecodeSource, PipelineRows};
use zenpixels::{PixelDescriptor, PixelSlice};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn make(path: &str, width: u32, height: u32) -> Result<(), Error> {
    let mut state = 0x1234_5678u32;
    let samples: Vec<u8> = (0..width as usize * height as usize * 3)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            // Half noise, half gradient: compressible but not trivially.
            if i % 2 == 0 {
                (state >> 24) as u8
            } else {
                (i / 3 % 256) as u8
            }
        })
        .collect();
    let pixels = PixelSlice::new(
        &samples,
        width,
        height,
        width as usize * 3,
        PixelDescriptor::RGB8_SRGB,
    )?;
    let file = zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)?
        .into_vec();
    std::fs::write(path, file)?;
    Ok(())
}

fn run(
    input: &str,
    output: &str,
    effort: i32,
    schedule: &str,
    spool: Option<&str>,
) -> Result<(), Error> {
    let codecs = CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new());
    if schedule == "oneshot" {
        let bytes = std::fs::read(input)?;
        let decoded = DecodeRequest::new().decode(&codecs, &bytes)?;
        let out = zenpng::PngEncoderConfig::new()
            .with_generic_effort(effort)
            .job()
            .encode(decoded.output().pixels())?
            .into_vec();
        std::fs::write(output, &out)?;
        println!("effort {effort} oneshot: emitted {} B", out.len());
        return Ok(());
    }
    let decode_budget = StorageBudget::new(1 << 30, 1024);
    let source = DecodeSource::new(
        &codecs,
        &DecodeRequest::new(),
        FileReadAt::open(input)?,
        PolicySet::new(),
        DecodeOptions::new(decode_budget.clone()).with_drive(DriveOptions::new(0, 1 << 20)),
    )?;
    let rows = PipelineRows::from_source(source)?;
    let encode_budget = StorageBudget::new(1 << 30, 1 << 16);
    let mut options = Options::new(encode_budget.clone(), 1 << 32);
    let required = match schedule {
        "live" => OutputSchedule::Live,
        "memory" => OutputSchedule::FinishFirst,
        "disk" => {
            let path = spool.ok_or("disk needs a spool path")?;
            let file = std::fs::File::options()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(path)?;
            options = options.with_disk_staging(file, NonZeroUsize::new(64 * 1024).unwrap())?;
            OutputSchedule::FinishFirst
        }
        other => return Err(format!("unknown schedule {other}").into()),
    };
    let policy = PolicySet::new().require_output_schedule(required);
    let done = row_encode::encode_job_to_destination(
        zenpng::PngEncoderConfig::new()
            .with_generic_effort(effort)
            .job(),
        rows,
        Forward(std::fs::File::create(output)?),
        policy,
        options.with_policy_set(policy),
        &NoPulse,
    )?;
    println!(
        "effort {effort} {schedule}: emitted {} B, schedule {:?}, managed decode/encode bytes still reserved {} / {}",
        done.emitted,
        done.schedule,
        decode_budget.reserved_bytes(),
        encode_budget.reserved_bytes()
    );
    Ok(())
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("make") if args.len() == 5 => make(&args[2], args[3].parse()?, args[4].parse()?),
        Some("run") if args.len() >= 6 => run(
            &args[2],
            &args[3],
            args[4].parse()?,
            &args[5],
            args.get(6).map(String::as_str),
        ),
        _ => Err(
            "usage: memory_probe make PATH W H | run IN OUT EFFORT live|memory|disk|oneshot [SPOOL]"
                .into(),
        ),
    }
}
