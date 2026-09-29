use super::*;

const XMP_KEY: &[u8] = b"XML:com.adobe.xmp";

pub(super) fn plan<'a>(req: &ScrubRequest<'a>) -> Result<Plan<'a>, Error> {
    let mut out = Plan::new();
    out.keep(req, 0..8, "PNG signature")?;
    let mut pos = 8;
    let mut seen = Vec::<[u8; 4]>::new();
    let mut color = 0;
    let mut depth = 0;
    let mut width = 0;
    let mut height = 0;
    let mut palette = 0;
    let mut idat = false;
    let mut idat_ended = false;
    let mut frames = None;
    let mut frame_count = 0u32;
    let mut sequence = 0u32;
    let mut frame_uses_idat = false;
    let mut xmp_seen = false;
    let mut frame_has_data = true;
    let mut cicp = None;
    let mut mastering = false;
    while pos < req.input.len() {
        req.check()?;
        let len = be32(&req.input[pos..])? as usize;
        if len > 0x7fff_ffff {
            return Err(Error::Malformed("PNG chunk size"));
        }
        let end = pos
            .checked_add(12)
            .and_then(|n| n.checked_add(len))
            .filter(|&n| n <= req.input.len())
            .ok_or(Error::Malformed("truncated PNG chunk"))?;
        let kind: [u8; 4] = req.input[pos + 4..pos + 8].try_into().unwrap();
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err(Error::Malformed("PNG chunk type"));
        }
        let data = &req.input[pos + 8..end - 4];
        let range = pos..end;
        let crc = crc32(&req.input[pos + 4..end - 4], req.stop)?;
        if crc != be32(&req.input[end - 4..end])? {
            return Err(Error::Malformed("PNG CRC"));
        }
        if pos == 8 && kind != *b"IHDR" {
            return Err(Error::Malformed("PNG IHDR must be first"));
        }
        let repeated = matches!(
            &kind,
            b"IDAT" | b"fdAT" | b"fcTL" | b"tEXt" | b"zTXt" | b"iTXt" | b"sPLT"
        );
        let known_single = matches!(
            &kind,
            b"IHDR"
                | b"PLTE"
                | b"IEND"
                | b"acTL"
                | b"eXIf"
                | b"iCCP"
                | b"sRGB"
                | b"cICP"
                | b"cHRM"
                | b"gAMA"
                | b"mDCV"
                | b"cLLI"
                | b"tRNS"
                | b"bKGD"
                | b"sBIT"
                | b"pHYs"
                | b"tIME"
                | b"hIST"
        );
        if known_single && seen.contains(&kind) {
            return Err(Error::Malformed("duplicate PNG singleton chunk"));
        }
        if !repeated && known_single {
            seen.push(kind);
        }
        if idat && kind != *b"IDAT" {
            idat_ended = true;
        }
        let before_palette = matches!(
            &kind,
            b"iCCP" | b"sRGB" | b"cICP" | b"cHRM" | b"gAMA" | b"sBIT" | b"mDCV" | b"cLLI"
        );
        if before_palette && (palette != 0 || idat) {
            return Err(Error::Malformed("late PNG color chunk"));
        }
        let before_data = before_palette
            || matches!(
                &kind,
                b"PLTE" | b"tRNS" | b"bKGD" | b"hIST" | b"pHYs" | b"acTL" | b"mDCV" | b"cLLI"
            );
        if before_data && idat {
            return Err(Error::Malformed("late PNG header chunk"));
        }
        match &kind {
            b"IHDR" => {
                if len != 13 {
                    return Err(Error::Malformed("PNG IHDR length"));
                }
                width = be32(data)?;
                height = be32(&data[4..])?;
                depth = data[8];
                color = data[9];
                let valid = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(depth, 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    _ => false,
                };
                if width == 0
                    || height == 0
                    || width > 0x7fff_ffff
                    || height > 0x7fff_ffff
                    || !valid
                    || data[10] != 0
                    || data[11] != 0
                    || data[12] > 1
                {
                    return Err(Error::Malformed("PNG IHDR values"));
                }
                out.keep(req, range, "PNG image header")?;
            }
            b"PLTE" => {
                if len == 0
                    || !len.is_multiple_of(3)
                    || len > 768
                    || matches!(color, 0 | 4)
                    || (color == 3 && len / 3 > 1usize << depth)
                {
                    return Err(Error::Malformed("PNG palette"));
                }
                palette = len / 3;
                out.keep(req, range, "PNG palette")?;
            }
            b"IDAT" => {
                if idat_ended || (color == 3 && palette == 0) {
                    return Err(Error::Malformed("PNG IDAT order/palette"));
                }
                idat = true;
                frame_has_data = true;
                out.keep(req, range, "PNG image data")?;
            }
            b"IEND" => {
                if len != 0 || !idat || !frame_has_data || frames.is_some_and(|n| n != frame_count)
                {
                    return Err(Error::Malformed("incomplete PNG/APNG"));
                }
                if mastering && cicp.is_none() {
                    return Err(Error::Malformed("PNG HDR envelope without cICP"));
                }
                if seen.contains(b"iCCP") && seen.contains(b"sRGB") {
                    return Err(Error::Malformed("PNG iCCP and sRGB conflict"));
                }
                out.keep(req, range, "PNG end")?;
                if end < req.input.len() {
                    out.remove(req, end..req.input.len(), "PNG trailer")?;
                }
                return Ok(out);
            }
            b"eXIf" => {
                let exif = req.exif(data)?;
                let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(&exif);
                out.record(
                    req,
                    range,
                    "EXIF",
                    Action::Rewrite,
                    Some(Cow::Owned(chunk(&kind, tiff, req.stop)?)),
                )?;
            }
            b"iCCP" => {
                req.packet(len)?;
                if matches!(req.policy.fields().icc, zencodec::IccRetention::Drop) {
                    return Err(Error::Unsupported(
                        "removing PNG ICC requires color conversion",
                    ));
                }
                let zero = data
                    .iter()
                    .position(|&b| b == 0)
                    .filter(|&i| (1..=79).contains(&i))
                    .ok_or(Error::Malformed("PNG ICC name"))?;
                if data.get(zero + 1) != Some(&0) || data.len() <= zero + 2 {
                    return Err(Error::Malformed("PNG ICC compression"));
                }
                if req.normalize_icc {
                    let profile = req.decompress(Compression::Zlib, &data[zero + 2..])?;
                    if let Some(normalized) = req.icc(&profile)? {
                        let compressed = req.services()?.compress(
                            Compression::Zlib,
                            &normalized,
                            req.max_metadata,
                            req.stop,
                        )?;
                        req.packet(compressed.len())?;
                        let body = [b"ICC\0\0".as_slice(), &compressed].concat();
                        out.record(
                            req,
                            range,
                            "ICC",
                            Action::Rewrite,
                            Some(Cow::Owned(chunk(&kind, &body, req.stop)?)),
                        )?;
                    } else {
                        let body = [b"ICC\0\0".as_slice(), &data[zero + 2..]].concat();
                        out.record(
                            req,
                            range,
                            "ICC (opaque, name normalized)",
                            Action::OpaqueIcc,
                            Some(Cow::Owned(chunk(&kind, &body, req.stop)?)),
                        )?;
                    }
                } else {
                    // The profile name is descriptive text outside the opaque
                    // profile. Normalize that header without inflating the profile.
                    let body = [b"ICC\0\0".as_slice(), &data[zero + 2..]].concat();
                    out.record(
                        req,
                        range,
                        "ICC (opaque, name normalized)",
                        Action::OpaqueIcc,
                        Some(Cow::Owned(chunk(&kind, &body, req.stop)?)),
                    )?;
                }
            }
            b"iTXt" if data.starts_with(XMP_KEY) && data.get(XMP_KEY.len()) == Some(&0) => {
                if xmp_seen && req.edit_xmp {
                    return Err(Error::Malformed("duplicate PNG XMP"));
                }
                xmp_seen = true;
                if !req.edit_xmp {
                    out.remove(req, range, "XMP")?;
                } else {
                    let xml = read_xmp(req, data)?;
                    if let Some(xml) = req.xmp(&xml)? {
                        let body = [XMP_KEY, b"\0\0\0\0\0", &xml].concat();
                        out.record(
                            req,
                            range,
                            "XMP",
                            Action::Rewrite,
                            Some(Cow::Owned(chunk(b"iTXt", &body, req.stop)?)),
                        )?;
                    } else {
                        out.remove(req, range, "XMP")?;
                    }
                }
            }
            b"sRGB" => {
                if len != 1 || data[0] > 3 {
                    return Err(Error::Malformed("PNG sRGB"));
                }
                out.keep(req, range, "sRGB")?;
            }
            b"cICP" => {
                if len != 4 || data[2] != 0 || data[3] != 1 {
                    return Err(Error::Malformed("PNG cICP matrix/range"));
                }
                // Unknown primaries/transfers remain unknown and unchanged.
                cicp = Some([data[0], data[1], data[2], data[3]]);
                out.keep(req, range, "CICP (authoritative)")?;
            }
            b"gAMA" => {
                if len != 4 || be32(data)? == 0 {
                    return Err(Error::Malformed("PNG gamma"));
                }
                out.keep(req, range, "PNG gamma")?;
            }
            b"cHRM" => {
                if len != 32 {
                    return Err(Error::Malformed("PNG chromaticities"));
                }
                out.keep(req, range, "PNG chromaticities")?;
            }
            b"mDCV" => {
                if len != 24 {
                    return Err(Error::Malformed("PNG mastering display"));
                }
                mastering = true;
                out.keep(req, range, "HDR mastering display")?;
            }
            b"cLLI" => {
                if len != 8 {
                    return Err(Error::Malformed("PNG content light level"));
                }

                out.keep(req, range, "HDR content light level")?;
            }
            b"sBIT" => {
                let n = match color {
                    0 => 1,
                    2 | 3 => 3,
                    4 => 2,
                    6 => 4,
                    _ => 0,
                };
                let max = if color == 3 { 8 } else { depth };
                if len != n || data.iter().any(|&v| v == 0 || v > max) {
                    return Err(Error::Malformed("PNG significant bits"));
                }
                out.keep(req, range, "PNG significant bits")?;
            }
            b"tRNS" => {
                let valid = match color {
                    0 => len == 2,
                    2 => len == 6,
                    3 => len > 0 && len <= palette,
                    _ => false,
                };
                if !valid {
                    return Err(Error::Malformed("PNG transparency"));
                }
                out.keep(req, range, "PNG transparency")?;
            }
            b"bKGD" => {
                let valid = match color {
                    0 | 4 => len == 2,
                    2 | 6 => len == 6,
                    3 => len == 1 && (data[0] as usize) < palette,
                    _ => false,
                };
                if !valid {
                    return Err(Error::Malformed("PNG background"));
                }
                out.keep(req, range, "PNG suggested background")?;
            }
            b"pHYs" => {
                if len != 9 || data[8] > 1 {
                    return Err(Error::Malformed("PNG pixel dimensions"));
                }
                out.keep(req, range, "PNG pixel aspect/density")?;
            }
            b"acTL" => {
                if len != 8 {
                    return Err(Error::Malformed("APNG control"));
                }
                let n = be32(data)?;
                if n == 0 {
                    return Err(Error::Malformed("APNG zero frames"));
                }
                frames = Some(n);
                out.keep(req, range, "APNG animation control")?;
            }
            b"fcTL" => {
                if frames.is_none() || len != 26 || !frame_has_data || be32(data)? != sequence {
                    return Err(Error::Malformed("APNG frame order"));
                }
                sequence = sequence
                    .checked_add(1)
                    .ok_or(Error::Limit("APNG sequence"))?;
                let w = be32(&data[4..])?;
                let h = be32(&data[8..])?;
                let x = be32(&data[12..])?;
                let y = be32(&data[16..])?;
                if w == 0
                    || h == 0
                    || w.checked_add(x).is_none_or(|v| v > width)
                    || h.checked_add(y).is_none_or(|v| v > height)
                    || data[24] > 2
                    || data[25] > 1
                    || (!idat && (x != 0 || y != 0 || w != width || h != height))
                {
                    return Err(Error::Malformed("APNG frame geometry/operation"));
                }
                frame_uses_idat = !idat;
                frame_count += 1;
                frame_has_data = false;
                out.keep(req, range, "APNG frame control")?;
            }
            b"fdAT" => {
                if !idat
                    || frame_uses_idat
                    || frames.is_none()
                    || frame_count == 0
                    || len < 4
                    || be32(data)? != sequence
                {
                    return Err(Error::Malformed("APNG frame data order"));
                }
                sequence = sequence
                    .checked_add(1)
                    .ok_or(Error::Limit("APNG sequence"))?;
                frame_has_data = true;
                out.keep(req, range, "APNG frame data")?;
            }
            _ if kind[0].is_ascii_uppercase() => {
                return Err(Error::Unsupported("unknown critical PNG chunk"));
            }
            _ => out.remove(req, range, "PNG descriptive/unknown ancillary chunk")?,
        }
        pos = end;
    }
    Err(Error::Malformed("missing PNG IEND"))
}

fn read_xmp(req: &ScrubRequest<'_>, data: &[u8]) -> Result<Vec<u8>, Error> {
    req.packet(data.len())?;
    let start = XMP_KEY.len() + 1;
    let flag = *data.get(start).ok_or(Error::Malformed("PNG XMP flags"))?;
    if flag > 1 || data.get(start + 1) != Some(&0) {
        return Err(Error::Malformed("PNG XMP compression"));
    }
    let mut rest = data
        .get(start + 2..)
        .ok_or(Error::Malformed("PNG XMP header"))?;
    for _ in 0..2 {
        let zero = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Malformed("PNG XMP language"))?;
        rest = &rest[zero + 1..];
    }
    if flag == 1 {
        req.decompress(Compression::Zlib, rest)
    } else {
        Ok(rest.to_vec())
    }
}

pub(super) fn chunk(
    kind: &[u8; 4],
    data: &[u8],
    stop: &dyn enough::Stop,
) -> Result<Vec<u8>, Error> {
    let len = u32::try_from(data.len()).map_err(|_| Error::Limit("PNG chunk size"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(data.len() + 12)
        .map_err(|_| Error::Allocation)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[4..], stop)?;
    out.extend_from_slice(&crc.to_be_bytes());
    Ok(out)
}
const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            j += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};
fn crc32(data: &[u8], stop: &dyn enough::Stop) -> Result<u32, Error> {
    let mut c = !0u32;
    for block in data.chunks(65536) {
        stop.check().map_err(|_| Error::Cancelled)?;
        for &b in block {
            c = CRC_TABLE[((c ^ b as u32) & 255) as usize] ^ (c >> 8);
        }
    }
    Ok(!c)
}
