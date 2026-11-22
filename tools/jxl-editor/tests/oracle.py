"""Compare browser artifacts using separately built JPEG XL decoders."""

import os
from pathlib import Path
import re
import struct
import subprocess
import sys

from PIL import Image, ImageSequence

directory = Path(sys.argv[1])
djxl = os.environ["DJXL"]
jxl_rs = os.environ["JXL_RS"]
identity = subprocess.check_output([djxl, "--version"], text=True)
if not re.search(r"\bdjxl v0\.12\.0 a7a9c78\b", identity):
    raise ValueError(f"Unexpected djxl build\n{identity}")

cases = [
    ("gradient.project", "gradient", (127, 93)),
    ("gradient", "gradient", (127, 93)),
    ("odd-strided", "odd-strided", (3, 2)),
    ("native-source", "native-source", (64, 41)),
    ("native.project", "native", (64, 41)),
]
for name, expected, size in cases:
    source = directory / f"{name}.jxl"
    expected_pixels = (directory / f"{expected}.rgba").read_bytes()
    for decoder, executable, threads in [
        ("djxl", djxl, "--num_threads=1"),
        ("jxl-rs", jxl_rs, "--num-threads=1"),
    ]:
        destination = directory / f"{name}.{decoder}.png"
        subprocess.run([executable, str(source), str(destination), threads], check=True)
        with Image.open(destination) as image:
            if image.size != size:
                raise ValueError(f"{name} {decoder} dimensions {image.size}, expected {size}")
            actual = image.convert("RGBA").tobytes()
        if actual != expected_pixels:
            different = sum(left != right for left, right in zip(actual, expected_pixels, strict=True))
            raise ValueError(f"{name} {decoder} has {different} differing RGBA bytes")
        print(f"{name} {decoder} matched {len(actual)} RGBA bytes")


def read_npy_f32(path: Path):
    raw = path.read_bytes()
    if raw[:6] != b"\x93NUMPY":
        raise ValueError(f"{path} is not a NumPy file")
    major = raw[6]
    if major == 1:
        hlen = struct.unpack("<H", raw[8:10])[0]
        offset = 10 + hlen
    else:
        hlen = struct.unpack("<I", raw[8:12])[0]
        offset = 12 + hlen
    hdr = raw[offset - hlen:offset].decode("latin1")
    m = re.search(r"'shape':\s*\(([^)]+)\)", hdr)
    shape = tuple(int(x.strip()) for x in m.group(1).split(",") if x.strip())
    body = raw[offset:]
    count = len(body) // 4
    vals = struct.unpack(f"<{count}f", body)
    return shape, vals


# 1. Native Timeline animation checks (all frames, exact RGBA bytes, exact durations & loop count)
if (directory / "timeline.jxl").exists():
    expected_frames = [(directory / f"timeline-f{i}.rgba").read_bytes() for i in range(3)]
    expected_durations = [150, 250, 350]
    for name in ("timeline.project", "timeline"):
        source = directory / f"{name}.jxl"
        # djxl v0.12: decode APNG and verify all 3 frames, durations, loop count, and RGBA bytes
        djxl_apng = directory / f"{name}.djxl.apng"
        subprocess.run([djxl, str(source), str(djxl_apng), "--num_threads=1"], check=True)
        with Image.open(djxl_apng) as img:
            frames = []
            durations = []
            for frame in ImageSequence.Iterator(img):
                if frame.size != (127, 93):
                    raise ValueError(f"{name} djxl frame size {frame.size}, expected (127, 93)")
                durations.append(round(frame.info.get("duration", 0)))
                frames.append(frame.convert("RGBA").tobytes())
            loop = img.info.get("loop")
        if len(frames) != 3 or durations != expected_durations or loop != 2:
            raise ValueError(f"{name} djxl timing mismatch: frames={len(frames)} durations={durations} loop={loop}")
        for idx, (act, exp) in enumerate(zip(frames, expected_frames, strict=True)):
            if act != exp:
                diff = sum(a != b for a, b in zip(act, exp, strict=True))
                raise ValueError(f"{name} djxl frame {idx} has {diff} differing RGBA bytes")
        print(f"{name} djxl matched all 3 timeline frames, durations {durations} ms, loop={loop}")

        # jxl-rs: verify timing from APNG acTL/fcTL chunks and all 3 frames' exact pixels from .npy
        # (upstream jxl_cli f370be4 emits IDAT rather than fdAT on frames > 0 in APNG).
        jxlrs_apng = directory / f"{name}.jxl-rs.apng"
        jxlrs_npy = directory / f"{name}.jxl-rs.npy"
        subprocess.run([jxl_rs, str(source), str(jxlrs_apng), "--num-threads=1"], check=True)
        subprocess.run([jxl_rs, str(source), str(jxlrs_npy), "--num-threads=1"], check=True)
        apng_raw = jxlrs_apng.read_bytes()
        pos = 8
        jxlrs_loop = None
        jxlrs_num_frames = None
        jxlrs_durations = []
        while pos < len(apng_raw):
            clen = struct.unpack(">I", apng_raw[pos:pos + 4])[0]
            ctype = apng_raw[pos + 4:pos + 8]
            cdata = apng_raw[pos + 8:pos + 8 + clen]
            if ctype == b"acTL":
                jxlrs_num_frames, jxlrs_loop = struct.unpack(">II", cdata[:8])
            elif ctype == b"fcTL":
                _, _, _, _, _, d_num, d_den, _, _ = struct.unpack(">IIIIIHHBB", cdata[:26])
                jxlrs_durations.append(round(d_num * 1000.0 / (d_den or 100)))
            pos += 12 + clen
        if jxlrs_num_frames != 3 or jxlrs_loop != 2 or jxlrs_durations != expected_durations:
            raise ValueError(
                f"{name} jxl-rs timing mismatch: frames={jxlrs_num_frames} durations={jxlrs_durations} loop={jxlrs_loop}"
            )
        shape, vals = read_npy_f32(jxlrs_npy)
        if shape != (3, 93, 127, 4):
            raise ValueError(f"{name} jxl-rs .npy shape {shape}, expected (3, 93, 127, 4)")
        frame_vals = 93 * 127 * 4
        for idx, exp in enumerate(expected_frames):
            sl = vals[idx * frame_vals:(idx + 1) * frame_vals]
            act = bytes(max(0, min(255, round(v * 255.0))) for v in sl)
            if act != exp:
                diff = sum(a != b for a, b in zip(act, exp, strict=True))
                raise ValueError(f"{name} jxl-rs frame {idx} has {diff} differing RGBA bytes")
        print(f"{name} jxl-rs matched all 3 timeline frames, durations {jxlrs_durations} ms, loop={jxlrs_loop}")

# 2. Manual Splines checks (visible rendered spline curve vs dark background in both decoders)
if (directory / "splines.jxl").exists():
    for name in ("splines.project", "splines"):
        source = directory / f"{name}.jxl"
        for decoder, executable, threads in [
            ("djxl", djxl, "--num_threads=1"),
            ("jxl-rs", jxl_rs, "--num-threads=1"),
        ]:
            dest = directory / f"{name}.{decoder}.png"
            subprocess.run([executable, str(source), str(dest), threads], check=True)
            with Image.open(dest) as img:
                if img.size != (96, 64):
                    raise ValueError(f"{name} {decoder} size {img.size}, expected (96, 64)")
                rgb = img.convert("RGB")
                mid = rgb.getpixel((48, 16))
                corner = rgb.getpixel((2, 2))
            if not (mid[0] > 140 and mid[1] > 120):
                raise ValueError(f"{name} {decoder} midpoint (48,16) not bright yellow: {mid}")
            if not (corner[0] < 40 and corner[1] < 45):
                raise ValueError(f"{name} {decoder} corner (2,2) not dark background: {corner}")
            print(f"{name} {decoder} verified spline midpoint={mid}, corner={corner}")

# 3. Coding modes checks (VarDCT progressive+LF+noise, VarDCT resampling 2x, Modular squeeze+128+YCoCg)
if (directory / "coding-modular.jxl").exists():
    mod_expected = (directory / "coding-modular.rgba").read_bytes()
    for name in ("coding-vardct", "coding-resamp", "coding-modular"):
        source = directory / f"{name}.jxl"
        for decoder, executable, threads in [
            ("djxl", djxl, "--num_threads=1"),
            ("jxl-rs", jxl_rs, "--num-threads=1"),
        ]:
            dest = directory / f"{name}.{decoder}.png"
            subprocess.run([executable, str(source), str(dest), threads], check=True)
            with Image.open(dest) as img:
                if img.size != (64, 48):
                    raise ValueError(f"{name} {decoder} size {img.size}, expected (64, 48)")
                if name == "coding-modular":
                    actual = img.convert("RGBA").tobytes()
                    if actual != mod_expected:
                        diff = sum(a != b for a, b in zip(actual, mod_expected, strict=True))
                        raise ValueError(f"{name} {decoder} has {diff} differing RGBA bytes")
            print(f"{name} {decoder} decoded 64x48 cleanly")

# 4. CMYK checks (Level 10 lossless exact Cmyk8 + exact ICC, and lossy CMYK ICC + Black extra channel)
if (directory / "cmyk-lossless.jxl").exists():
    expected_cmyk = (directory / "cmyk-lossless.cmyk8").read_bytes()
    expected_icc = (directory / "cmyk-expected.icc").read_bytes()
    for name, exact_samples in [
        ("cmyk-lossless.project", True),
        ("cmyk-lossless", True),
        ("cmyk-lossy", False),
    ]:
        source = directory / f"{name}.jxl"
        djxl_png = directory / f"{name}.djxl.png"
        djxl_icc = directory / f"{name}.djxl.icc"
        subprocess.run(
            [djxl, str(source), str(djxl_png), "--orig_icc_out", str(djxl_icc), "--num_threads=1"],
            check=True,
        )
        if djxl_icc.read_bytes() != expected_icc:
            raise ValueError(f"{name} djxl ICC mismatch")
        with Image.open(djxl_png) as img:
            if img.size != (24, 16):
                raise ValueError(f"{name} djxl size {img.size}, expected (24, 16)")
            rgb = img.convert("RGB")
            white_px = rgb.getpixel((4, 4))
            black_px = rgb.getpixel((12, 12))
            if not (white_px[0] > 235 and white_px[1] > 235 and white_px[2] > 235):
                raise ValueError(f"{name} djxl white swatch unexpected: {white_px}")
            if not (black_px[0] < 20 and black_px[1] < 20 and black_px[2] < 20):
                raise ValueError(f"{name} djxl black swatch unexpected: {black_px}")

        jxlrs_npy = directory / f"{name}.jxl-rs.npy"
        jxlrs_icc = directory / f"{name}.jxl-rs.icc"
        subprocess.run(
            [
                jxl_rs, "--allow-level10", str(source), str(jxlrs_npy),
                "--original-icc-out", str(jxlrs_icc), "--num-threads=1",
            ],
            check=True,
        )
        if jxlrs_icc.read_bytes() != expected_icc:
            raise ValueError(f"{name} jxl-rs ICC mismatch")
        shape, vals = read_npy_f32(jxlrs_npy)
        if shape != (1, 16, 24, 4):
            raise ValueError(f"{name} jxl-rs .npy shape {shape}, expected (1, 16, 24, 4)")
        if exact_samples:
            actual_u8 = bytes(max(0, min(255, round(v * 255.0))) for v in vals)
            if actual_u8 != expected_cmyk:
                diff = sum(a != b for a, b in zip(actual_u8, expected_cmyk, strict=True))
                raise ValueError(f"{name} jxl-rs CMYK8 samples differ in {diff} bytes")
            print(f"{name} djxl+jxl-rs matched all {len(actual_u8)} CMYK8 codes + {len(expected_icc)} B ICC")
        else:
            # Verify Black extra channel (index 3) survives in lossy CMYK: white K~1.0, black K~0.0
            white_k = vals[(4 * 24 + 4) * 4 + 3]
            black_k = vals[(12 * 24 + 12) * 4 + 3]
            if not (white_k > 0.95 and black_k < 0.05):
                raise ValueError(f"{name} jxl-rs lossy Black channel unexpected: white_k={white_k}, black_k={black_k}")
            print(f"{name} djxl+jxl-rs verified lossy CMYK ICC ({len(expected_icc)} B) + Black K channel (white={white_k:.3f}, black={black_k:.3f})")
