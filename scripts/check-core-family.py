#!/usr/bin/env python3
"""Fail if any graph in this repo holds more than one copy of the zencodec core family.

The core family is zencodec, zencodec-io, zencodec-coroutine and zenmeta. Their
traits and types must unify: a native codec compiled against one copy does not
implement the traits of another, and the failure surfaces far from its cause.
Since 2026-10-28 every graph root here names the family by version and patches
all four crates to one revision in `[patch.crates-io]`. A dependency that still
names a family crate by git revision brings its own copy, because a patch cannot
redirect a git source to another revision of the same repository.

This reads the committed lockfiles, which cover every feature combination, so
it needs no network and no build. It checks, per graph root:

  1. ONE COPY   each family package appears at most once among resolved packages;
  2. ONE SOURCE every family package present comes from the same source;

and, across graph roots, that they all name that same source. Unused patch
entries (`[[patch.unused]]`) are not resolved packages and are ignored.

Usage: check-core-family.py [--root DIR] [--self-test]
"""
import argparse
import pathlib
import shutil
import sys
import tomllib

FAMILY = ("zencodec", "zencodec-io", "zencodec-coroutine", "zenmeta")
GRAPH_ROOTS = (
    "Cargo.lock",
    "fuzz/Cargo.lock",
    "demo/crate/Cargo.lock",
    "tools/metadata-services/Cargo.lock",
    "zencodecs/fuzz/Cargo.lock",
)


def family_sources(lock_path):
    """Map each family package present to the list of its resolved sources."""
    found = {}
    for package in tomllib.loads(lock_path.read_text()).get("package", []):
        if package["name"] in FAMILY:
            found.setdefault(package["name"], []).append(package.get("source", "path"))
    return found


def check(root):
    problems = []
    graph_sources = {}
    for relative in GRAPH_ROOTS:
        lock = root / relative
        if not lock.exists():
            problems.append(f"{relative}: missing")
            continue
        found = family_sources(lock)
        sources = set()
        for name, copies in sorted(found.items()):
            if len(copies) != 1:
                problems.append(f"{relative}: {name} has {len(copies)} copies: {copies}")
            sources.update(copies)
        if len(sources) > 1:
            problems.append(f"{relative}: family split across {sorted(sources)}")
        elif sources:
            graph_sources[relative] = sources.pop()
    if len(set(graph_sources.values())) > 1:
        problems.append(f"graph roots disagree: {graph_sources}")
    return problems, graph_sources


def lock_text(entries):
    out = ["version = 4", ""]
    for name, source in entries:
        out += ["[[package]]", f'name = "{name}"', 'version = "0.1.0"']
        if source:
            out.append(f'source = "{source}"')
        out.append("")
    return "\n".join(out)


def self_test(workdir):
    a = "git+https://github.com/imazen/zencodec?rev=aaaa#aaaa"
    b = "git+https://github.com/imazen/zencodec?rev=bbbb#bbbb"
    cases = [
        ("one family everywhere", [("zencodec", a), ("zenmeta", a)], [("zencodec", a)], True),
        ("two zencodec copies", [("zencodec", a), ("zencodec", b)], [("zencodec", a)], False),
        ("family split in one graph", [("zencodec", a), ("zenmeta", b)], [("zencodec", a)], False),
        ("graphs disagree", [("zencodec", a)], [("zencodec", b)], False),
        ("no family at all", [("other", a)], [("other", a)], True),
    ]
    ok = True
    for index, (name, first, rest, want) in enumerate(cases):
        root = workdir / f"case-{index}"
        shutil.rmtree(root, ignore_errors=True)
        for i, relative in enumerate(GRAPH_ROOTS):
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(lock_text(first if i == 0 else rest))
        problems, _ = check(root)
        got = not problems
        ok &= got == want
        print(f"self-test {'ok' if got == want else 'WRONG'}: {name}")
    shutil.rmtree(workdir, ignore_errors=True)
    return ok


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=pathlib.Path(__file__).resolve().parent.parent, type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        if not self_test(args.root / "target" / "check-core-family-selftest"):
            print("self-test FAILED")
            return 1
        print("self-test OK")
        return 0
    problems, graph_sources = check(args.root)
    for problem in problems:
        print(f"FAIL: {problem}")
    if problems:
        return 1
    source = next(iter(graph_sources.values()), "none")
    print(f"OK -- one core family in all {len(GRAPH_ROOTS)} graph roots: {source}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
