#!/usr/bin/env python3
"""Summarise `zencodecs inventory --tsv [--unconsumed]` output for a privacy audit.

Usage: summarize_inventory.py inventory.tsv [--out summary.md]

Reads the TSV rows (file, offset, length, depth, kind, tag, disposition, label,
detail) and writes a Markdown summary:

- per format, files, and file-level status (clean / unsupported / error / ...);
- unconsumed bytes and parts per disposition;
- every distinct (kind, tag, label) that is unconsumed, with file counts and an
  example path, sorted by how many files carry it: this is the list to read for
  PII, private chunks and trailers;
- the files with the most unconsumed bytes.

Labels and details are printed as found (they come from the files). Nothing
is decoded or interpreted here.
"""
import argparse
import collections
import csv
import os
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("tsv")
    ap.add_argument("--out", default="-")
    ap.add_argument("--top", type=int, default=25)
    a = ap.parse_args()

    csv.field_size_limit(sys.maxsize)
    status = collections.Counter()
    by_disp = collections.defaultdict(lambda: [0, 0])  # disposition -> [parts, bytes]
    sigs = collections.defaultdict(lambda: {"files": set(), "parts": 0, "bytes": 0, "example": None, "disp": collections.Counter()})
    per_file = collections.Counter()
    files = set()
    ext = collections.Counter()

    with open(a.tsv, newline="") as f:
        r = csv.DictReader(f, delimiter="\t")
        for row in r:
            path = row["file"]
            if path not in files:
                files.add(path)
                ext[os.path.splitext(path)[1].lower() or "(none)"] += 1
            if row["kind"] == "file":
                status[row["disposition"]] += 1
                continue
            disp = row["disposition"]
            if disp in ("structure", "image-data") or disp.startswith("metadata("):
                continue
            n = int(row["length"])
            by_disp[disp][0] += 1
            by_disp[disp][1] += n
            per_file[path] += n
            key = (row["kind"], row["tag"], row["label"])
            s = sigs[key]
            s["files"].add(path)
            s["parts"] += 1
            s["bytes"] += n
            s["disp"][disp] += 1
            if s["example"] is None:
                s["example"] = f"{path} @ {row['offset']} ({row['detail']})"

    out = sys.stdout if a.out == "-" else open(a.out, "w")
    w = lambda s="": print(s, file=out)
    w(f"# Inventory audit summary\n\nSource: `{a.tsv}`, {len(files)} files.\n")
    w("| extension | files |\n|---|--:|")
    for e, c in ext.most_common():
        w(f"| {e} | {c} |")
    w("\n| file status | files |\n|---|--:|")
    for s_, c in status.most_common():
        w(f"| {s_} | {c} |")
    w("\n## Unconsumed parts by disposition\n\n| disposition | parts | bytes |\n|---|--:|--:|")
    for d, (p, b) in sorted(by_disp.items(), key=lambda kv: -kv[1][1]):
        w(f"| {d} | {p} | {b} |")
    w("\n## Distinct unconsumed units (read these for PII)\n")
    w("| kind | tag | label | files | parts | bytes | dispositions | example |\n|---|---|---|--:|--:|--:|---|---|")
    for (kind, tag, label), s in sorted(sigs.items(), key=lambda kv: (-len(kv[1]["files"]), -kv[1]["bytes"])):
        disps = ", ".join(f"{k}×{v}" for k, v in s["disp"].most_common())
        lab = label.replace("|", "\\|")
        ex = (s["example"] or "").replace("|", "\\|")
        w(f"| {kind} | {tag} | {lab} | {len(s['files'])} | {s['parts']} | {s['bytes']} | {disps} | {ex} |")
    w(f"\n## Files with the most unconsumed bytes (top {a.top})\n\n| file | bytes |\n|---|--:|")
    for p, b in per_file.most_common(a.top):
        w(f"| {p} | {b} |")


if __name__ == "__main__":
    main()
