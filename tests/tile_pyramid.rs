//! Filesystem / zip output of `TilePyramidSink` through every layout
//! (zenpipe#24).

#![cfg(feature = "std")]

whereat::define_at_crate_info!();

use std::path::PathBuf;
use zenpipe::sources::MaterializedSource;
use zenpipe::tiles::{
    DziLayout, FsStore, GoogleMapsLayout, Iiif3Layout, MemoryStore, PyramidWriter, TileLayout,
    TilePyramidConfig, TilePyramidSink, TileRef, TileStore, ZipStore, ZoomifyLayout,
};
use zenpipe::{PipeResult, Source, execute, format};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("tile_pyramid")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn source(w: u32, h: u32) -> Box<dyn Source> {
    let data: Vec<u8> = (0..w * h)
        .flat_map(|i| [i as u8, (i >> 8) as u8, 7, 255])
        .collect();
    Box::new(MaterializedSource::from_data(
        data,
        w,
        h,
        format::RGBA8_SRGB,
    ))
}

/// "Encoder": raw packed bytes with a tiny header — the codec is the
/// caller's choice; these tests check layout, not compression.
fn raw_encode(t: TileRef<'_>) -> PipeResult<Vec<u8>> {
    let mut v = t.width.to_le_bytes().to_vec();
    v.extend_from_slice(&t.height.to_le_bytes());
    v.extend_from_slice(t.data);
    Ok(v)
}

fn run<L: TileLayout, S: TileStore>(
    w: u32,
    h: u32,
    cfg: TilePyramidConfig,
    writer: PyramidWriter<L, S>,
) -> (Vec<zenpipe::tiles::LevelInfo>, PyramidWriter<L, S>) {
    let mut src = source(w, h);
    let mut sink =
        TilePyramidSink::with_pyramid_writer(w, h, format::RGBA8_SRGB, cfg, writer).unwrap();
    execute(src.as_mut(), &mut sink).unwrap();
    let levels = sink.level_infos();
    (levels, sink.into_writer())
}

#[test]
fn dzi_fs_writes_descriptor_and_every_tile() {
    let (w, h) = (300u32, 200u32);
    let dir = scratch("dzi");
    let cfg = TilePyramidConfig::new(64, 1);
    let writer = PyramidWriter::new(DziLayout::new("img", "raw"), FsStore::new(&dir), raw_encode);
    let (levels, writer) = run(w, h, cfg, writer);

    let dzi = std::fs::read_to_string(dir.join("img.dzi")).unwrap();
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

#[test]
fn iiif3_fs_layout() {
    let (w, h) = (300u32, 200u32);
    let dir = scratch("iiif3");
    let cfg = TilePyramidConfig {
        tile_size: 128,
        ..TilePyramidConfig::iiif()
    };
    let writer = PyramidWriter::new(
        Iiif3Layout::new("pic", "raw").with_service_id("https://example.org/iiif/pic"),
        FsStore::new(&dir),
        raw_encode,
    );
    let (levels, writer) = run(w, h, cfg, writer);
    let info = std::fs::read_to_string(dir.join("pic/info.json")).unwrap();
    assert!(
        info.contains("\"id\":\"https://example.org/iiif/pic\""),
        "{info}"
    );
    assert!(info.contains("\"width\":300,\"height\":200"), "{info}");
    assert!(
        info.contains("\"tiles\":[{\"width\":128,\"height\":128"),
        "{info}"
    );
    // Full res: 3×2 tiles; the bottom-right one is 44×72 at region (256,128).
    let n = levels.len() as u32 - 1;
    assert_eq!(n, 9);
    let br = std::fs::read(dir.join("pic/256,128,44,72/44,72/0/default.raw")).unwrap();
    assert_eq!(br.len(), 8 + 44 * 72 * 4);
    // Half res (150×100): tile (1,0) → region x=256 w=44 (full-res px), 22 px wide.
    assert!(dir.join("pic/256,0,44,200/22,100/0/default.raw").exists());
    // Apex.
    assert!(dir.join("pic/0,0,300,200/1,1/0/default.raw").exists());
    let total: u64 = levels
        .iter()
        .map(|l| u64::from(l.width.div_ceil(128)) * u64::from(l.height.div_ceil(128)))
        .sum();
    assert_eq!(writer.tiles_written(), total);
}

#[test]
fn zoomify_fs_layout_groups_256_tiles() {
    // 3000×100 at 32 px tiles: 94×4 = 376 tiles at full res alone, so the
    // numbering spans more than one TileGroup.
    let (w, h) = (3000u32, 100u32);
    let dir = scratch("zoomify");
    let cfg = TilePyramidConfig {
        tile_size: 32,
        ..TilePyramidConfig::zoomify()
    };
    let writer = PyramidWriter::new(ZoomifyLayout::new("raw"), FsStore::new(&dir), raw_encode);
    let (levels, writer) = run(w, h, cfg, writer);
    // 3000 → 1500 → 750 → 375 → 188 → 94 → 47 → 24 (≤ 32, height 1) → 8 levels.
    assert_eq!(levels.len(), 8);
    assert_eq!(levels[0].width, 24);
    let total: u64 = levels
        .iter()
        .map(|l| u64::from(l.width.div_ceil(32)) * u64::from(l.height.div_ceil(32)))
        .sum();
    assert_eq!(writer.tiles_written(), total);
    let props = std::fs::read_to_string(dir.join("ImageProperties.xml")).unwrap();
    assert!(props.contains(&format!("NUMTILES=\"{total}\"")), "{props}");
    assert!(props.contains("WIDTH=\"3000\" HEIGHT=\"100\""), "{props}");
    // Apex is tile 0.
    assert!(dir.join("TileGroup0/0-0-0.raw").exists());
    // Tiles below the full level: 1 + 2 + 3 + 6 + 12 + 24 + 47 = 95 (each
    // level is one tile row until 94 px is 3 rows ... count from geometry).
    let below: u64 = levels[..7]
        .iter()
        .map(|l| u64::from(l.width.div_ceil(32)) * u64::from(l.height.div_ceil(32)))
        .sum();
    // Full level tile (col 5, row 2): number = below + 2·94 + 5.
    let num = below + 2 * 94 + 5;
    let group = num / 256;
    assert!(
        dir.join(format!("TileGroup{group}/7-5-2.raw")).exists(),
        "expected TileGroup{group} for tile {num}"
    );
    let groups = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("TileGroup")
        })
        .count() as u64;
    assert_eq!(groups, total.div_ceil(256));
    // No group holds more than 256 tiles.
    for g in 0..groups {
        let n = std::fs::read_dir(dir.join(format!("TileGroup{g}")))
            .unwrap()
            .count();
        assert!(n <= 256, "TileGroup{g} has {n}");
    }
}

#[test]
fn google_maps_fs_layout_pads_to_complete_tiles_and_skips_blanks() {
    let (w, h) = (300u32, 200u32);
    let dir = scratch("google");
    let bg = [1, 2, 3, 255];
    let cfg = TilePyramidConfig {
        tile_size: 128,
        ..TilePyramidConfig::google_maps(bg)
    };
    let writer = PyramidWriter::new(GoogleMapsLayout::new("raw"), FsStore::new(&dir), raw_encode)
        .with_skip_blanks(bg, 0);
    let (levels, writer) = run(w, h, cfg, writer);
    // 300 → 512 canvas: z0 = 128, z1 = 256, z2 = 512 (4×4 tiles).
    assert_eq!(levels.len(), 3);
    assert_eq!(levels[2].width, 512);
    // Every stored tile is a complete 128×128.
    for z in 0..3u32 {
        for entry in std::fs::read_dir(dir.join(z.to_string())).unwrap() {
            for tile in std::fs::read_dir(entry.unwrap().path()).unwrap() {
                let bytes = std::fs::read(tile.unwrap().path()).unwrap();
                assert_eq!(&bytes[..4], &128u32.to_le_bytes());
                assert_eq!(bytes.len(), 8 + 128 * 128 * 4);
            }
        }
    }
    // z2: columns 3 (x ≥ 384 > 300) and rows 2..3 (y ≥ 256 > 200) are pure
    // background → skipped. Present: cols 0..3 × rows 0..2 = 6 tiles.
    assert!(dir.join("2/0/0.raw").exists());
    assert!(dir.join("2/1/2.raw").exists());
    assert!(!dir.join("2/0/3.raw").exists(), "blank column stored");
    assert!(!dir.join("2/2/0.raw").exists(), "blank row stored");
    // z1 (256 canvas, 150×100 content): tiles (0,0),(1,0) have content;
    // (0,1),(1,1) are blank. z0: one tile.
    assert_eq!(writer.tiles_skipped(), (16 - 6) + 2);
    assert_eq!(writer.tiles_written(), 6 + 2 + 1);
}

/// Minimal zip reader: walks the central directory (ZIP64 when present)
/// and returns `(name, crc32, size, stored bytes)`.
fn read_zip(bytes: &[u8]) -> Vec<(String, u32, u32, Vec<u8>)> {
    let le32 = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let le16 = |o: usize| u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap());
    let le64 = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
    let eocd = bytes.len() - 22;
    assert_eq!(le32(eocd), 0x0605_4b50, "EOCD signature");
    let (mut count, mut cd_off) = (u64::from(le16(eocd + 10)), u64::from(le32(eocd + 16)));
    if count == 0xFFFF || cd_off == 0xFFFF_FFFF {
        let loc = eocd - 20;
        assert_eq!(le32(loc), 0x0706_4b50, "zip64 locator");
        let z64 = le64(loc + 8) as usize;
        assert_eq!(le32(z64), 0x0606_4b50, "zip64 EOCD");
        count = le64(z64 + 32);
        cd_off = le64(z64 + 48);
    }
    let mut out = Vec::new();
    let mut p = cd_off as usize;
    for _ in 0..count {
        assert_eq!(le32(p), 0x0201_4b50, "central header");
        let crc = le32(p + 16);
        let size = le32(p + 20);
        let name_len = usize::from(le16(p + 28));
        let extra_len = usize::from(le16(p + 30));
        let mut local = u64::from(le32(p + 42));
        let name = String::from_utf8(bytes[p + 46..p + 46 + name_len].to_vec()).unwrap();
        if local == 0xFFFF_FFFF {
            let e = p + 46 + name_len;
            assert_eq!(le16(e), 1);
            local = le64(e + 4);
        }
        let l = local as usize;
        assert_eq!(le32(l), 0x0403_4b50, "local header");
        let lname = usize::from(le16(l + 26));
        let lextra = usize::from(le16(l + 28));
        let data = bytes[l + 30 + lname + lextra..l + 30 + lname + lextra + size as usize].to_vec();
        out.push((name, crc, size, data));
        p += 46 + name_len + extra_len;
    }
    out
}

#[test]
fn zip_store_holds_the_same_files_as_the_memory_store() {
    let (w, h) = (300u32, 200u32);
    let cfg = TilePyramidConfig::new(64, 1);
    let (_, mem) = run(
        w,
        h,
        cfg,
        PyramidWriter::new(
            DziLayout::new("img", "raw"),
            MemoryStore::default(),
            raw_encode,
        ),
    );
    let (_, zip) = run(
        w,
        h,
        cfg,
        PyramidWriter::new(
            DziLayout::new("img", "raw"),
            ZipStore::new(Vec::new()),
            raw_encode,
        ),
    );
    let bytes = zip.into_store().into_inner().unwrap();
    let entries = read_zip(&bytes);
    let mem = mem.into_store();
    assert_eq!(entries.len(), mem.files.len());
    for (name, crc, size, data) in &entries {
        let expected = mem
            .files
            .get(name)
            .unwrap_or_else(|| panic!("extra zip entry {name}"));
        assert_eq!(data, expected, "{name} bytes");
        assert_eq!(*size as usize, expected.len());
        assert_eq!(*crc, crc32_ref(expected), "{name} crc");
    }
    assert!(mem.files.contains_key("img.dzi"));
}

/// Independent bitwise CRC-32 for the zip test.
fn crc32_ref(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn zip_store_emits_zip64_records_past_65535_entries() {
    // Direct store use: 70 000 tiny entries.
    let mut store = ZipStore::new(Vec::new());
    for i in 0..70_000u32 {
        store.put(&format!("t/{i}"), &i.to_le_bytes()).unwrap();
    }
    let bytes = store.into_inner().unwrap();
    let entries = read_zip(&bytes);
    assert_eq!(entries.len(), 70_000);
    assert_eq!(entries[69_999].0, "t/69999");
    assert_eq!(entries[69_999].3, 69_999u32.to_le_bytes());
    // Every entry is byte-checked by the reader (local header + data).
    assert!(entries.iter().all(|(_, _, s, d)| *s == 4 && d.len() == 4));
}

#[test]
fn parallel_row_encoding_matches_sequential() {
    let (w, h) = (900u32, 300u32);
    let cfg = TilePyramidConfig::new(64, 1);
    let (_, seq) = run(
        w,
        h,
        cfg,
        PyramidWriter::new(
            DziLayout::new("p", "raw"),
            MemoryStore::default(),
            raw_encode,
        ),
    );
    let (_, par) = run(
        w,
        h,
        cfg,
        PyramidWriter::new(
            DziLayout::new("p", "raw"),
            MemoryStore::default(),
            raw_encode,
        )
        .with_threads(3),
    );
    assert_eq!(seq.tiles_written(), par.tiles_written());
    assert_eq!(seq.into_store().files, par.into_store().files);
}

#[test]
fn encode_errors_propagate_from_worker_threads() {
    let (w, h) = (300u32, 100u32);
    let cfg = TilePyramidConfig::new(32, 0);
    let mut src = source(w, h);
    let writer = PyramidWriter::new(DziLayout::new("e", "raw"), MemoryStore::default(), |t| {
        if t.col == 3 && t.level == 9 {
            Err(whereat::at!(zenpipe::PipeError::Op("boom".into())))
        } else {
            raw_encode(t)
        }
    })
    .with_threads(4);
    let mut sink =
        TilePyramidSink::with_pyramid_writer(w, h, format::RGBA8_SRGB, cfg, writer).unwrap();
    let err = execute(src.as_mut(), &mut sink).unwrap_err();
    assert!(format!("{err}").contains("boom"), "{err}");
}
