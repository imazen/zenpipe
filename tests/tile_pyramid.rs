//! DZI filesystem output of `TilePyramidSink` (zenpipe#24).

#![cfg(feature = "std")]

use std::path::PathBuf;
use zenpipe::sources::MaterializedSource;
use zenpipe::tiles::{DziFsWriter, TilePyramidConfig, TilePyramidSink};
use zenpipe::{Source, execute, format};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("tile_pyramid")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn writes_dzi_descriptor_and_every_tile() {
    let (w, h) = (300u32, 200u32);
    let data: Vec<u8> = (0..w * h)
        .flat_map(|i| [i as u8, (i >> 8) as u8, 7, 255])
        .collect();
    let mut src: Box<dyn Source> = Box::new(MaterializedSource::from_data(
        data,
        w,
        h,
        format::RGBA8_SRGB,
    ));
    let dir = scratch("basic");
    let cfg = TilePyramidConfig::new(64, 1);
    // "Encoder": raw packed bytes with a tiny header — the codec is the
    // caller's choice; this test checks the layout, not compression.
    let writer = DziFsWriter::new(&dir, "img", "raw", |t| {
        let mut v = t.width.to_le_bytes().to_vec();
        v.extend_from_slice(&t.height.to_le_bytes());
        v.extend_from_slice(t.data);
        Ok(v)
    });
    let mut sink = TilePyramidSink::new(w, h, format::RGBA8_SRGB, cfg, writer).unwrap();
    execute(src.as_mut(), &mut sink).unwrap();
    let levels = sink.level_infos();
    let writer = sink.into_writer();

    let dzi = std::fs::read_to_string(writer.descriptor_path()).unwrap();
    assert!(dzi.contains("TileSize=\"64\""), "{dzi}");
    assert!(dzi.contains("Overlap=\"1\""), "{dzi}");
    assert!(dzi.contains("Format=\"raw\""), "{dzi}");
    assert!(
        dzi.contains("Width=\"300\"") && dzi.contains("Height=\"200\""),
        "{dzi}"
    );

    // ceil(log2(300)) = 9 → levels 0..=9.
    assert_eq!(levels.len(), 10);
    let mut expected_tiles = 0u64;
    for l in &levels {
        let n = u64::from(l.width.div_ceil(64)) * u64::from(l.height.div_ceil(64));
        let level_dir = dir.join("img_files").join(l.level.to_string());
        let count = std::fs::read_dir(&level_dir).unwrap().count() as u64;
        assert_eq!(count, n, "level {} tile count", l.level);
        expected_tiles += n;
    }
    assert_eq!(writer.tiles_written(), expected_tiles);

    // Spot-check the full-res top-left tile: 65×65 (64 + 1 overlap right/bottom).
    let top = std::fs::read(dir.join("img_files/9/0_0.raw")).unwrap();
    assert_eq!(&top[..4], &65u32.to_le_bytes());
    assert_eq!(&top[4..8], &65u32.to_le_bytes());
    assert_eq!(top.len(), 8 + 65 * 65 * 4);
    // Apex is a single pixel.
    let apex = std::fs::read(dir.join("img_files/0/0_0.raw")).unwrap();
    assert_eq!(apex.len(), 8 + 4);
}
