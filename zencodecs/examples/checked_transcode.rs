//! Convert a primary JPEG file to sRGB PNG with explicit metadata exclusion.
use std::sync::Arc;
use zencodec::{
    CodecSet, ImageFormat,
    operation::{MetadataTransfer, PolicySet},
};
use zencodecs::checked_transcode::TranscodeRequest;
use zenpixels::PixelFormat;
use zenpixels_convert::{ConvertPolicy, cms::MoxCms};

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
        .with_cms(Arc::new(MoxCms))
        .with_output(PixelFormat::RGB8_SRGB, ConvertPolicy::lossless())
        .with_metadata(MetadataTransfer::DiscardDescriptive);
    let mut policy = PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false);
    let mut source = zencodec_io::FileReadAt::open(input)?;
    let encoded = request.transcode_from_read_at_with(&codecs, &mut source, &mut policy)?;
    std::fs::write(output, encoded.data())?;
    Ok(())
}
