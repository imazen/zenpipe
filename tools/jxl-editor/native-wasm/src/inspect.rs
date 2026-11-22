//! Minimal spec-compliant ISO/IEC 18181-1 FrameHeader and container inspector.
//!
//! Used by `native-wasm` alongside `zenjxl-decoder` so the Native JXL inspector
//! and tests can observe coding-level bitstream flags (`encoding`, `flags`,
//! `ENABLE_SPLINES`, `ENABLE_PATCHES`, `ENABLE_NOISE`, `USE_LF_FRAME`,
//! `do_ycbcr`, `jpeg_upsampling`, `upsampling`, `group_size_shift`,
//! `num_passes`, `gaborish`, `epf_iters`, `jxll` level).

use serde_json::{Value, json};

pub const FLAG_ENABLE_NOISE: u64 = 0x01;
pub const FLAG_ENABLE_PATCHES: u64 = 0x02;
pub const FLAG_ENABLE_SPLINES: u64 = 0x10;
pub const FLAG_USE_LF_FRAME: u64 = 0x20;
pub const FLAG_SKIP_ADAPTIVE_LF_SMOOTHING: u64 = 0x80;

const CONTAINER_SIG: [u8; 12] = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 135, 10];

pub struct ContainerSummary {
    pub is_container: bool,
    pub codestream_level: u8,
    pub codestream: Vec<u8>,
    /// Mapping `(file_start, codestream_start, len)` for contiguous chunks of `jxlc`/`jxlp`.
    pub chunks: Vec<(u64, usize, usize)>,
}

/// Extract the codestream and container level (`jxll`) from either a bare codestream
/// or an ISO/IEC 18181-2 box container.
pub fn extract_codestream(bytes: &[u8]) -> Option<ContainerSummary> {
    if bytes.starts_with(&[0xFF, 0x0A]) {
        return Some(ContainerSummary {
            is_container: false,
            codestream_level: 5,
            codestream: bytes.to_vec(),
            chunks: vec![(0, 0, bytes.len())],
        });
    }
    if bytes.len() < 12 || bytes[..12] != CONTAINER_SIG {
        return None;
    }
    let mut offset = 12usize;
    let mut level = 5u8;
    let mut codestream = Vec::new();
    let mut chunks = Vec::new();
    let mut jxlp_parts: Vec<(u32, u64, &[u8])> = Vec::new();

    while offset + 8 <= bytes.len() {
        let raw_len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?);
        let box_type = &bytes[offset + 4..offset + 8];
        let (header_len, total_len) = if raw_len == 0 {
            (8usize, bytes.len() - offset)
        } else if raw_len == 1 {
            if offset + 16 > bytes.len() {
                break;
            }
            let ext = u64::from_be_bytes(bytes[offset + 8..offset + 16].try_into().ok()?);
            (16usize, usize::try_from(ext).ok()?)
        } else {
            (8usize, raw_len as usize)
        };
        if total_len < header_len || offset + total_len > bytes.len() {
            break;
        }
        let data_start = offset + header_len;
        let data = &bytes[data_start..offset + total_len];
        match box_type {
            b"jxll" if !data.is_empty() => {
                level = data[0];
            }
            b"jxlc" => {
                let cs_start = codestream.len();
                codestream.extend_from_slice(data);
                chunks.push((data_start as u64, cs_start, data.len()));
            }
            b"jxlp" if data.len() >= 4 => {
                let idx = u32::from_be_bytes(data[0..4].try_into().ok()?) & 0x7FFF_FFFF;
                jxlp_parts.push((idx, (data_start + 4) as u64, &data[4..]));
            }
            _ => {}
        }
        offset += total_len;
    }
    if codestream.is_empty() && !jxlp_parts.is_empty() {
        jxlp_parts.sort_by_key(|(idx, _, _)| *idx);
        for (_, file_pos, part) in jxlp_parts {
            let cs_start = codestream.len();
            codestream.extend_from_slice(part);
            chunks.push((file_pos, cs_start, part.len()));
        }
    }
    Some(ContainerSummary {
        is_container: true,
        codestream_level: level,
        codestream,
        chunks,
    })
}

impl ContainerSummary {
    /// Map a file byte offset (from `VisibleFrameInfo::file_offset`) into the
    /// concatenated codestream byte offset.
    pub fn file_to_codestream_offset(&self, file_offset: u64) -> Option<usize> {
        for &(file_start, cs_start, len) in &self.chunks {
            if file_offset >= file_start && file_offset < file_start + len as u64 {
                return Some(cs_start + (file_offset - file_start) as usize);
            }
        }
        None
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bits(&mut self, n: u32) -> Option<u64> {
        if n == 0 {
            return Some(0);
        }
        if n > 56 {
            return None;
        }
        let end_bit = self.bit_pos.checked_add(n as usize)?;
        if end_bit > self.data.len() * 8 {
            return None;
        }
        let mut out = 0u64;
        for i in 0..n as usize {
            let bp = self.bit_pos + i;
            let bit = (self.data[bp >> 3] >> (bp & 7)) & 1;
            out |= u64::from(bit) << i;
        }
        self.bit_pos = end_bit;
        Some(out)
    }

    fn read_bool(&mut self) -> Option<bool> {
        Some(self.read_bits(1)? != 0)
    }

    fn read_u32(&mut self, spec: [(u32, u32); 4]) -> Option<u32> {
        let sel = self.read_bits(2)? as usize;
        let (bits, off) = spec[sel];
        let v = self.read_bits(bits)? as u32;
        v.checked_add(off)
    }

    fn read_u64(&mut self) -> Option<u64> {
        match self.read_bits(2)? {
            0 => Some(0),
            1 => Some(self.read_bits(4)? + 1),
            2 => Some(self.read_bits(8)? + 17),
            _ => {
                let mut val = self.read_bits(12)?;
                let mut shift = 12u32;
                while self.read_bool()? {
                    if shift == 60 {
                        val |= self.read_bits(4)? << shift;
                        break;
                    }
                    val |= self.read_bits(8)? << shift;
                    shift += 8;
                }
                Some(val)
            }
        }
    }

    fn read_f16_skip(&mut self) -> Option<()> {
        self.read_bits(16)?;
        Some(())
    }

    fn read_extensions_skip(&mut self) -> Option<()> {
        let ext = self.read_u64()?;
        if ext == 0 {
            return Some(());
        }
        let mut total_bits = 0u64;
        for b in 0..64 {
            if (ext >> b) & 1 != 0 {
                total_bits = total_bits.checked_add(self.read_u64()?)?;
            }
        }
        let mut rem = total_bits;
        while rem > 0 {
            let take = rem.min(32) as u32;
            self.read_bits(take)?;
            rem -= u64::from(take);
        }
        Some(())
    }
}

pub struct FrameHeaderContext {
    pub xyb_encoded: bool,
    pub num_extra_channels: u32,
    pub img_width: u32,
    pub img_height: u32,
    pub have_animation: bool,
    pub have_timecodes: bool,
}

fn read_blending_info(
    br: &mut BitReader<'_>,
    num_extra_channels: u32,
    full_frame: bool,
) -> Option<u32> {
    let mode = br.read_u32([(0, 0), (0, 1), (0, 2), (2, 3)])?;
    if num_extra_channels > 0 && (mode == 2 || mode == 3) {
        let _alpha_ch = br.read_u32([(0, 0), (0, 1), (0, 2), (3, 3)])?;
    }
    if (num_extra_channels > 0 && (mode == 2 || mode == 3)) || mode == 4 {
        let _clamp = br.read_bool()?;
    }
    if !(full_frame && mode == 0) {
        let _source = br.read_bits(2)?;
    }
    Some(mode)
}

fn unpack_signed(v: u32) -> i32 {
    if v & 1 == 0 {
        (v >> 1) as i32
    } else {
        -(((v >> 1) as i32) + 1)
    }
}

/// Parse a single ISO/IEC 18181-1 `FrameHeader` starting at `slice[0]` and return a
/// JSON object with its coding flags, passes, resampling, YCbCr, and loop-filter state.
pub fn parse_frame_header_json(slice: &[u8], ctx: &FrameHeaderContext) -> Option<Value> {
    let mut br = BitReader::new(slice);
    let all_default = br.read_bool()?;
    if all_default {
        return Some(json!({
            "allDefault": true,
            "frameType": "Regular",
            "encoding": "VarDCT",
            "flags": {
                "raw": 0,
                "noise": false,
                "patches": false,
                "splines": false,
                "useLfFrame": false,
                "skipAdaptiveLfSmoothing": false
            },
            "doYcbcr": false,
            "jpegUpsampling": [0, 0, 0],
            "chromaSubsampling": "4:4:4",
            "upsampling": 1,
            "groupSizeShift": null,
            "xQmScale": 3,
            "bQmScale": 2,
            "numPasses": 1,
            "lfLevel": 0,
            "gaborish": true,
            "epfIters": 2
        }));
    }

    let frame_type_id = br.read_bits(2)? as u8;
    let frame_type = match frame_type_id {
        0 => "Regular",
        1 => "LfFrame",
        2 => "ReferenceOnly",
        _ => "SkipProgressive",
    };
    let encoding_id = br.read_bits(1)? as u8;
    let encoding = if encoding_id == 0 {
        "VarDCT"
    } else {
        "Modular"
    };
    let flags = br.read_u64()?;
    let do_ycbcr = if !ctx.xyb_encoded {
        br.read_bool()?
    } else {
        false
    };
    let mut jpeg_upsampling = [0u32; 3];
    if do_ycbcr && (flags & FLAG_USE_LF_FRAME == 0) {
        for v in &mut jpeg_upsampling {
            *v = br.read_bits(2)? as u32;
        }
    }
    let mut upsampling = 1u32;
    if flags & FLAG_USE_LF_FRAME == 0 {
        upsampling = br.read_u32([(0, 1), (0, 2), (0, 4), (0, 8)])?;
        for _ in 0..ctx.num_extra_channels {
            let _ec_up = br.read_u32([(0, 1), (0, 2), (0, 4), (0, 8)])?;
        }
    }
    let group_size_shift = if encoding_id == 1 {
        Some(br.read_bits(2)? as u32)
    } else {
        None
    };
    let (x_qm_scale, b_qm_scale) = if encoding_id == 0 && ctx.xyb_encoded {
        (Some(br.read_bits(3)? as u32), Some(br.read_bits(3)? as u32))
    } else {
        (None, None)
    };

    let mut num_passes = 1u32;
    if frame_type_id != 2 {
        num_passes = br.read_u32([(0, 1), (0, 2), (0, 3), (3, 4)])?;
        if num_passes != 1 {
            let num_ds = br.read_u32([(0, 0), (0, 1), (0, 2), (1, 3)])?;
            for _ in 0..num_passes.saturating_sub(1) {
                let _shift = br.read_bits(2)?;
            }
            for _ in 0..num_ds {
                let _ds = br.read_u32([(0, 1), (0, 2), (0, 4), (0, 8)])?;
            }
            for _ in 0..num_ds {
                let _lp = br.read_u32([(0, 0), (0, 1), (0, 2), (3, 0)])?;
            }
        }
    }

    let lf_level = if frame_type_id == 1 {
        br.read_u32([(0, 1), (0, 2), (0, 3), (0, 4)])?
    } else {
        0
    };

    let mut have_crop = false;
    let mut x0 = 0i32;
    let mut y0 = 0i32;
    let mut frame_width = ctx.img_width;
    let mut frame_height = ctx.img_height;
    let crop_spec = [(8, 0), (11, 256), (14, 2304), (30, 18688)];
    if frame_type_id != 1 {
        have_crop = br.read_bool()?;
        if have_crop {
            if frame_type_id != 2 {
                x0 = unpack_signed(br.read_u32(crop_spec)?);
                y0 = unpack_signed(br.read_u32(crop_spec)?);
            }
            frame_width = br.read_u32(crop_spec)?;
            frame_height = br.read_u32(crop_spec)?;
        }
    }

    let completely_covers = x0 <= 0
        && y0 <= 0
        && (i64::from(frame_width) + i64::from(x0)) >= i64::from(ctx.img_width)
        && (i64::from(frame_height) + i64::from(y0)) >= i64::from(ctx.img_height);
    let full_frame = !have_crop || completely_covers;

    let normal_frame = frame_type_id == 0 || frame_type_id == 3;
    let mut blend_mode = 0u32;
    let mut duration = 0u32;
    let mut is_last = frame_type_id == 0;
    if normal_frame {
        blend_mode = read_blending_info(&mut br, ctx.num_extra_channels, full_frame)?;
        for _ in 0..ctx.num_extra_channels {
            let _ec_blend = read_blending_info(&mut br, ctx.num_extra_channels, full_frame)?;
        }
        if ctx.have_animation {
            duration = br.read_u32([(0, 0), (0, 1), (8, 0), (32, 0)])?;
            if ctx.have_timecodes {
                let _tc = br.read_bits(32)?;
            }
        }
        is_last = br.read_bool()?;
    }

    if frame_type_id != 1 && !is_last {
        let save_as_reference = br.read_bits(2)? as u32;
        let can_be_referenced = duration == 0 || save_as_reference != 0;
        let save_before_ct_def_false =
            can_be_referenced && blend_mode == 0 && full_frame && normal_frame;
        if frame_type_id == 2 || save_before_ct_def_false {
            let _save_before_ct = br.read_bool()?;
        }
    }

    // Frame name
    let name_len = br.read_u32([(0, 0), (4, 0), (5, 16), (10, 48)])? as usize;
    for _ in 0..name_len {
        br.read_bits(8)?;
    }

    // RestorationFilter
    let rf_all_default = br.read_bool()?;
    let (gaborish, epf_iters) = if rf_all_default {
        (true, 2u32)
    } else {
        let gab = br.read_bool()?;
        if gab && br.read_bool()? {
            for _ in 0..6 {
                br.read_f16_skip()?;
            }
        }
        let epf = br.read_bits(2)? as u32;
        if epf > 0 && encoding_id == 0 && br.read_bool()? {
            for _ in 0..8 {
                br.read_f16_skip()?;
            }
        }
        if epf > 0 && br.read_bool()? {
            for _ in 0..5 {
                br.read_f16_skip()?;
            }
        }
        if epf > 0 && br.read_bool()? {
            if encoding_id == 0 {
                br.read_f16_skip()?;
            }
            for _ in 0..3 {
                br.read_f16_skip()?;
            }
            if encoding_id == 1 {
                br.read_f16_skip()?;
            }
        }
        br.read_extensions_skip()?;
        (gab, epf)
    };

    let chroma_subsampling = if !do_ycbcr {
        "4:4:4"
    } else {
        match jpeg_upsampling {
            [0, 0, 0] => "4:4:4",
            [1, 0, 1] => "4:2:0",
            [2, 0, 2] => "4:2:2",
            [3, 0, 3] => "4:4:0",
            _ => "custom",
        }
    };

    Some(json!({
        "allDefault": false,
        "frameType": frame_type,
        "encoding": encoding,
        "flags": {
            "raw": flags,
            "noise": (flags & FLAG_ENABLE_NOISE) != 0,
            "patches": (flags & FLAG_ENABLE_PATCHES) != 0,
            "splines": (flags & FLAG_ENABLE_SPLINES) != 0,
            "useLfFrame": (flags & FLAG_USE_LF_FRAME) != 0,
            "skipAdaptiveLfSmoothing": (flags & FLAG_SKIP_ADAPTIVE_LF_SMOOTHING) != 0
        },
        "doYcbcr": do_ycbcr,
        "jpegUpsampling": jpeg_upsampling,
        "chromaSubsampling": chroma_subsampling,
        "upsampling": upsampling,
        "groupSizeShift": group_size_shift,
        "modularGroupSize": group_size_shift.map(|s| 128u32 << s),
        "xQmScale": x_qm_scale,
        "bQmScale": b_qm_scale,
        "numPasses": num_passes,
        "lfLevel": lf_level,
        "gaborish": gaborish,
        "epfIters": epf_iters
    }))
}
