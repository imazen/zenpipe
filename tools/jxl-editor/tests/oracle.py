"""Compare browser artifacts using separately built JPEG XL decoders."""

import os
from pathlib import Path
import re
import subprocess
import sys

from PIL import Image

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
