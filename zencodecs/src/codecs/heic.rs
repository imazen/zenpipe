//! Still-image container decode adapter (delegates to the backend via trait interface).

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, GainMapRender, ImageFormat, ImageInfo,
    Limits, OrientationHint, SharedPulse,
};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::Decode;

/// Probe container metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let dec = heic::HeicDecoderConfig::new();
    let job = dec.job();
    at_crate!(job.probe_full(data)).map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))
}

/// Decode container stills to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    pulse: SharedPulse,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
    gain_map_render: GainMapRender,
    extract_gain_map: bool,
    orientation: OrientationHint,
) -> Result<DecodeOutput> {
    let mut dec = heic::HeicDecoderConfig::new();
    if extract_gain_map {
        dec = dec.with_extract_gain_map(true);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    job = job.with_pulse(pulse);
    if let Some(dp) = decode_policy {
        // Refuse a policy the codec cannot enforce instead of ignoring it.
        job.try_set_policy(dp)
            .map_err(|e| whereat::at!(CodecError::from_codec(ImageFormat::Heic, e)))?;
    }
    // BaseOnly (default) is a no-op; ReconstructHdr makes the backend apply the gain map
    // and emit linear-float HDR pixels (via ultrahdr-core's reconstruction).
    job = job.with_gain_map_render(gain_map_render);
    // Preserve (default) leaves the EXIF tag authoritative; Correct bakes it.
    job = job.with_orientation(orientation);
    let decoder = at_crate!(job.decoder(Cow::Borrowed(data), &[]))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))?;
    at_crate!(decoder.decode()).map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use zencodec::decode::{DecodeJob as _, DecoderConfig as _};

    fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    fn full_box(typ: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version];
        p.extend_from_slice(&flags.to_be_bytes()[1..]);
        p.extend_from_slice(payload);
        bx(typ, &p)
    }

    fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    fn synthetic_heic_with_icc() -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let hdlr = full_box(b"hdlr", 0, 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let pitm = full_box(b"pitm", 0, 0, &1u16.to_be_bytes());
        let mut infe_p = 1u16.to_be_bytes().to_vec();
        infe_p.extend_from_slice(&[0, 0]);
        infe_p.extend_from_slice(b"hvc1");
        infe_p.extend_from_slice(b"Primary\0");
        let iinf = full_box(
            b"iinf",
            0,
            0,
            &cat(&[
                1u16.to_be_bytes().to_vec(),
                full_box(b"infe", 2, 0, &infe_p),
            ]),
        );

        let hvcc: &[u8] = &[
            1, 3, 112, 0, 0, 0, 0, 0, 0, 0, 0, 0, 60, 240, 0, 252, 253, 248, 248, 0, 0, 15, 3, 96,
            0, 1, 0, 24, 64, 1, 12, 1, 255, 255, 3, 112, 0, 0, 3, 0, 144, 0, 0, 3, 0, 0, 3, 0, 60,
            186, 2, 64, 97, 0, 1, 0, 44, 66, 1, 1, 3, 112, 0, 0, 3, 0, 144, 0, 0, 3, 0, 0, 3, 0,
            60, 160, 8, 8, 4, 5, 150, 234, 73, 41, 174, 110, 2, 26, 12, 8, 0, 0, 3, 0, 200, 0, 0,
            3, 0, 8, 64, 98, 0, 1, 0, 7, 68, 1, 193, 114, 176, 98, 64,
        ];

        let ipco = bx(
            b"ipco",
            &cat(&[
                bx(b"hvcC", hvcc),
                full_box(
                    b"ispe",
                    0,
                    0,
                    &cat(&[256u32.to_be_bytes().to_vec(), 256u32.to_be_bytes().to_vec()]),
                ),
                bx(
                    b"colr",
                    &cat(&[
                        b"prof".to_vec(),
                        20u32.to_be_bytes().to_vec(),
                        b"sixteen-icc-body".to_vec(),
                    ]),
                ),
                full_box(b"pixi", 0, 0, &[3, 8, 8, 8]),
            ]),
        );
        let ipma = full_box(
            b"ipma",
            0,
            0,
            &cat(&[1u32.to_be_bytes().to_vec(), vec![0, 1, 4, 0x81, 2, 3, 4]]),
        );
        let iprp = bx(b"iprp", &cat(&[ipco, ipma]));
        let mdat_payload = b"\x00\x00\x00\x04\x46\x01\x50\x00";
        let mdat = bx(b"mdat", mdat_payload);

        let make_iloc = |offset: u32, len: u32| -> Vec<u8> {
            let mut p = vec![0x44, 0x00];
            p.extend_from_slice(&1u16.to_be_bytes());
            p.extend_from_slice(&1u16.to_be_bytes());
            p.extend_from_slice(&[0, 0, 0, 0]);
            p.extend_from_slice(&1u16.to_be_bytes());
            p.extend_from_slice(&offset.to_be_bytes());
            p.extend_from_slice(&len.to_be_bytes());
            full_box(b"iloc", 1, 0, &p)
        };

        let meta_len = full_box(
            b"meta",
            0,
            0,
            &cat(&[
                hdlr.clone(),
                pitm.clone(),
                make_iloc(0, 0),
                iinf.clone(),
                iprp.clone(),
            ]),
        )
        .len();
        let data_offset = (ftyp.len() + meta_len + 8) as u32;
        let meta = full_box(
            b"meta",
            0,
            0,
            &cat(&[
                hdlr,
                pitm,
                make_iloc(data_offset, mdat_payload.len() as u32),
                iinf,
                iprp,
            ]),
        );
        cat(&[ftyp, meta, mdat])
    }

    #[test]
    fn heic_probe_uses_probe_full_to_preserve_icc_profile() {
        let bytes = synthetic_heic_with_icc();
        let light = heic::HeicDecoderConfig::new().job().probe(&bytes).unwrap();
        assert!(light.source_color.icc_profile.is_none());

        let full = crate::from_bytes(&bytes).unwrap();
        assert_eq!(full.format, ImageFormat::Heic);
        assert_eq!(full.width, 256);
        assert_eq!(full.height, 256);
        assert!(full.source_color.icc_profile.is_some());
    }
}
