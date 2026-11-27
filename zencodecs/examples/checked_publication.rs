//! Convert JPEG to sRGB PNG, then publish only verified output to a file.
use std::sync::Arc;
use zencodec::{
    CodecSet, ImageFormat,
    operation::{MetadataTransfer, OutputSchedule, PolicySet},
};
use zencodecs::checked_transcode::TranscodeRequest;
use zenpixels::PixelFormat;
use zenpixels_convert::{ConvertPolicy, cms::MoxCms};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args
        .next()
        .ok_or("usage: checked_publication INPUT.jpg OUTPUT.png")?;
    let output = args
        .next()
        .ok_or("usage: checked_publication INPUT.jpg OUTPUT.png")?;
    let codecs = CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
        .with_encoder(zenpng::PngEncoderConfig::new());
    let request = TranscodeRequest::new(ImageFormat::Png)
        .with_cms(Arc::new(MoxCms))
        .with_output(PixelFormat::RGB8_SRGB, ConvertPolicy::lossless())
        .with_metadata(MetadataTransfer::DiscardDescriptive);
    let mut policy = PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false)
        .require_output_schedule(OutputSchedule::FinishFirst);
    let input = std::fs::read(input)?;
    let ready = request.transcode_for_publication(&codecs, &input, &mut policy)?;
    drop(input);
    drop(codecs);
    let mut destination = std::fs::File::create(output)?;
    let _receipt = ready.write_to(&mut destination, None)?;
    Ok(())
}
