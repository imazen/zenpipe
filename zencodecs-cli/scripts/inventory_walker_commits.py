#!/usr/bin/env python3
"""Print the source and commit of every crate that provides an inventory walker, from Cargo.lock.

Usage: inventory_walker_commits.py [Cargo.lock]

Output is TSV (crate, version, source, commit) so an audit's results can name the exact walker
revisions they came from.
"""
import re
import sys

WALKERS = {
    "zencodec", "zenjpeg", "zenpng", "zenwebp", "zengif", "heic", "zenavif", "zenavif-parse",
    "zenjxl", "zenjxl-decoder", "zenbitmaps", "zenraw", "zentiff", "zenpdf", "zensvg",
}


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "Cargo.lock"
    text = open(path).read()
    print("crate\tversion\tsource\tcommit")
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        if not name or name.group(1) not in WALKERS:
            continue
        version = re.search(r'^version = "([^"]+)"', block, re.M).group(1)
        source = re.search(r'^source = "([^"]+)"', block, re.M)
        if not source:
            print(f"{name.group(1)}\t{version}\tpath\t-")
            continue
        src, _, commit = source.group(1).partition("#")
        print(f"{name.group(1)}\t{version}\t{src}\t{commit or '-'}")


if __name__ == "__main__":
    main()
