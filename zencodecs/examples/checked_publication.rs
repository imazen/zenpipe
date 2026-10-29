//! Convert JPEG to sRGB PNG, then publish only verified output to a file.
use zencodec::{
    CodecSet, ImageFormat,
    operation::{MetadataTransfer, OutputSchedule, PolicySet},
};
use zencodecs::checked_transcode::TranscodeRequest;
use zenpixels::{Cicp, ConvertOptions, PixelFormat};
use zenpixels_convert::{OutputProfile, cms_moxcms::MoxCms};

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
        .with_cms(&MoxCms)
        .with_output(
            OutputProfile::Named(Cicp::SRGB),
            PixelFormat::Rgb8,
            ConvertOptions::retain_channels(),
        )
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
