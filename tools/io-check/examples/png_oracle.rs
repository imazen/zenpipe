//! Exact-output oracle for the memory matrix: decodes PNG files with the
//! independent `png` crate (a test oracle, not zen code) and compares their
//! samples with the samples `memory_probe make` authors.
//!
//! `png_oracle W H FILE.png...` regenerates the authored W x H RGB8 samples
//! (the same generator as `memory_probe make`), then reports every file as
//! `equal` or `DIFFERENT`. Pass the fixture too: it checks the fixture's own
//! encoding. Exits nonzero when any file differs or fails to decode.
use std::io::Cursor;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Must stay identical to `memory_probe`'s `make` generator.
fn authored(width: u32, height: u32) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..width as usize * height as usize * 3)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if i % 2 == 0 {
                (state >> 24) as u8
            } else {
                (i / 3 % 256) as u8
            }
        })
        .collect()
}

/// Decoded samples as RGB8, with a description of the stored format.
fn rgb8(bytes: &[u8]) -> Result<(u32, u32, String, Vec<u8>), Error> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("output size overflows")?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let stored = format!(
        "{:?} {:?}",
        reader.info().color_type,
        reader.info().bit_depth
    );
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("decoded depth {:?} ({stored})", info.bit_depth).into());
    }
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf,
        png::ColorType::Rgba => {
            if buf.chunks_exact(4).any(|p| p[3] != 255) {
                return Err(format!("opaque samples gained alpha ({stored})").into());
            }
            buf.chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        }
        other => return Err(format!("decoded as {other:?} ({stored})").into()),
    };
    Ok((info.width, info.height, stored, rgb))
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [w, h, files @ ..] = &args[..] else {
        return Err("usage: png_oracle W H FILE.png...".into());
    };
    let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
    let expected = authored(w, h);
    let mut failed = 0;
    for file in files {
        let bytes = std::fs::read(file)?;
        match rgb8(&bytes) {
            Ok((fw, fh, stored, samples)) => {
                let equal = (fw, fh) == (w, h) && samples == expected;
                let differing = samples
                    .iter()
                    .zip(&expected)
                    .filter(|(a, b)| a != b)
                    .count();
                println!(
                    "{}\t{file}\t{fw}x{fh}\t{stored}\t{} B\t{differing} samples differ",
                    if equal { "equal" } else { "DIFFERENT" },
                    bytes.len()
                );
                if !equal {
                    failed += 1;
                }
            }
            Err(e) => {
                println!("ERROR\t{file}\t{e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(format!(
            "{failed} of {} files differ from the authored samples",
            files.len()
        )
        .into());
    }
    Ok(())
}
