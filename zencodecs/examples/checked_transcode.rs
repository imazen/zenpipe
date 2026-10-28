//! Convert a primary JPEG file to sRGB PNG with explicit metadata exclusion.
use zencodec::{
    CodecSet, ImageFormat,
    operation::{MetadataTransfer, PolicySet},
};
use zencodecs::checked_transcode::TranscodeRequest;
use zenpixels::{Cicp, ConvertOptions, PixelFormat};
use zenpixels_convert::{OutputProfile, cms_moxcms::MoxCms};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args
        .next()
        .ok_or("usage: checked_transcode INPUT.jpg OUTPUT.png")?;
    let output = args
        .next()
        .ok_or("usage: checked_transcode INPUT.jpg OUTPUT.png")?;
    let codecs = CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
        .with_encoder(zenpng::PngEncoderConfig::new());
    let request = TranscodeRequest::new(ImageFormat::Png)
        .with_cms(&MoxCms)
        .with_output(
            OutputProfile::Named(Cicp::SRGB),
            PixelFormat::Rgb8,
            ConvertOptions::forbid_lossy(),
        )
        .with_metadata(MetadataTransfer::DiscardDescriptive);
    let mut policy = PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false);
    let mut source = zencodec_io::FileReadAt::open(input)?;
    let encoded = request.transcode_from_read_at_with(&codecs, &mut source, &mut policy)?;
    std::fs::write(output, encoded.data())?;
    Ok(())
}
