use std::io::{Read, Write};
use zencodecs::metadata::ScrubRequest;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        return Err("usage: metadata-services INPUT OUTPUT [--keep-rights] [--normalize-icc] [--remove-jpeg-reconstruction]".into());
    }
    let keep = args.iter().any(|a| a == "--keep-rights");
    let services = zencodecs_metadata_services::PublishingServices { keep_rights: keep };
    let limit = 512usize << 20;
    let mut input = Vec::new();
    std::fs::File::open(&args[0])?
        .take(limit as u64 + 1)
        .read_to_end(&mut input)?;
    if input.len() > limit {
        return Err("input byte limit".into());
    }
    let request = ScrubRequest::new(&input)
        .with_services(&services)
        .with_policy(if keep {
            zencodec::MetadataPolicy::Web
        } else {
            zencodec::MetadataPolicy::ColorAndRotation
        })
        .with_xmp_editing(keep)
        .with_icc_normalization(args.iter().any(|a| a == "--normalize-icc"))
        .with_jpeg_reconstruction_removal(args.iter().any(|a| a == "--remove-jpeg-reconstruction"));
    let plan = request.plan()?;
    for entry in plan.report() {
        eprintln!(
            "{:?}: {} {:?}",
            entry.action(),
            entry.carrier(),
            entry.source_range()
        );
    }
    // Never replace an existing file. Planning is complete before output opens.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    plan.chunks()
        .try_for_each(|chunk| output.write_all(chunk))?;
    eprintln!(
        "{} encoded bytes; zero pixel passes; retained ICC/codestreams remain opaque",
        plan.output_len()
    );
    Ok(())
}
