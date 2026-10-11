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

Byte totals per disposition and per file count each byte once: an unconsumed
part nested inside another unconsumed part is subtracted from its parent, so
the totals partition the unconsumed bytes by their innermost disposition. The
per-unit table reports each unit's full length.

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

    def flush(rows):
        """Own bytes of each unconsumed part of one file: its length minus the
        lengths of the unconsumed parts directly inside it."""
        rows.sort(key=lambda t: (t[0], -t[1]))
        own = [n for (_, n, _, _) in rows]
        stack = []  # indices of open unconsumed parts
        for i, (start, n, _, _) in enumerate(rows):
            while stack and start >= rows[stack[-1]][0] + rows[stack[-1]][1]:
                stack.pop()
            if stack:
                own[stack[-1]] -= n
            stack.append(i)
        for (start, n, disp, path), o in zip(rows, own):
            by_disp[disp][1] += o
            per_file[path] += o

    pending = collections.defaultdict(list)  # path -> [(offset, length, disposition, path)]
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
            pending[path].append((int(row["offset"]), n, disp, path))
            key = (row["kind"], row["tag"], row["label"])
            s = sigs[key]
            s["files"].add(path)
            s["parts"] += 1
            s["bytes"] += n
            s["disp"][disp] += 1
            if s["example"] is None:
                s["example"] = f"{path} @ {row['offset']} ({row['detail']})"

    for rows in pending.values():
        flush(rows)

    out = sys.stdout if a.out == "-" else open(a.out, "w")
    w = lambda s="": print(s, file=out)
    w(f"# Inventory audit summary\n\nSource: `{a.tsv}`, {len(files)} files.\n")
    w("| extension | files |\n|---|--:|")
    for e, c in ext.most_common():
        w(f"| {e} | {c} |")
    w("\n| file status | files |\n|---|--:|")
    for s_, c in status.most_common():
        w(f"| {s_} | {c} |")
    w("\n## Unconsumed parts by disposition\n\nBytes count each byte once, under its innermost unconsumed part.\n\n| disposition | parts | bytes |\n|---|--:|--:|")
    for d, (p, b) in sorted(by_disp.items(), key=lambda kv: -kv[1][1]):
        w(f"| {d} | {p} | {b} |")
    w("\n## Distinct unconsumed units (read these for PII)\n")
    w("| kind | tag | label | files | parts | bytes (part lengths) | dispositions | example |\n|---|---|---|--:|--:|--:|---|---|")
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
