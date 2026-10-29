#!/usr/bin/env python3
"""Fail if any graph in this repo holds more than one copy of the zencodec core family.

The core family includes zencodec, its drivers/helpers/testkits and zenmeta. Their
traits and types must unify: a native codec compiled against one copy does not
implement the traits of another, and the failure surfaces far from its cause.
Every graph root names the four required members by version and patches them to
one revision in `[patch.crates-io]`. Other family packages are checked when present.
A dependency that still
names a family crate by git revision brings its own copy, because a patch cannot
redirect a git source to another revision of the same repository.

This reads source manifests and committed lockfiles without network or a build.
It checks, per graph root:

  1. ONE COPY   each family package appears at most once among resolved packages;
  2. ONE SOURCE every family package present comes from the same source;
  3. DECLARED SOURCE the lock agrees with that graph's current core patch;

and, across graph roots, that manifests and locks name the same source. Old locks
that agree with each other still refuse after the declared pin changes. Unused patch
entries (`[[patch.unused]]`) are not resolved packages and are ignored.

Usage: check-core-family.py [--root DIR] [--self-test]
"""
import argparse
import pathlib
import shutil
import sys
import tomllib

REQUIRED_PATCHES = ("zencodec", "zencodec-io", "zencodec-coroutine", "zenmeta")
FAMILY = REQUIRED_PATCHES + (
    "zencodec-testkit",
    "zencodec-media",
    "zencodec-media-testkit",
    "zencodec-render",
)
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


def declared_source(root):
    """Require one explicit core source in every portable consumer manifest."""
    problems = []
    expected = None
    for relative in GRAPH_ROOTS:
        manifest = pathlib.Path(relative).with_name("Cargo.toml")
        try:
            data = tomllib.loads((root / manifest).read_text())
            patches = data.get("patch", {}).get("crates-io", {})
        except (OSError, tomllib.TOMLDecodeError) as error:
            problems.append(f"{manifest}: cannot read core patches: {error}")
            continue
        core = patches.get("zencodec", {})
        if not isinstance(core, dict) or not core.get("git") or not core.get("rev"):
            problems.append(f"{manifest}: zencodec needs an explicit git revision")
            continue
        source = f'git+{core["git"]}?rev={core["rev"]}#{core["rev"]}'
        if expected is None:
            expected = source
        elif source != expected:
            problems.append(f"{manifest}: declared core differs from {expected}")
        for name in FAMILY:
            entry = patches.get(name)
            if entry is None and name not in REQUIRED_PATCHES:
                continue
            if not isinstance(entry, dict) or (
                entry.get("git"), entry.get("rev")
            ) != (core["git"], core["rev"]):
                problems.append(f"{manifest}: {name} patch differs from declared core")
    return problems, expected


def check(root):
    problems, expected = declared_source(root)
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
            if expected is not None:
                for source in copies:
                    if source != expected:
                        problems.append(
                            f"{relative}: {name} source {source} differs from declared {expected}"
                        )
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


def manifest_text(revision="aaaa"):
    patches = "\n".join(
        f'{name} = {{ git = "https://github.com/imazen/zencodec", rev = "{revision}" }}'
        for name in REQUIRED_PATCHES
    )
    return "[patch.crates-io]\n" + patches + "\n"


def write_case(root, first, rest, revision="aaaa"):
    for i, relative in enumerate(GRAPH_ROOTS):
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(lock_text(first if i == 0 else rest))
        path.with_name("Cargo.toml").write_text(manifest_text(revision))


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
        write_case(root, first, rest)
        problems, _ = check(root)
        got = not problems
        ok &= got == want
        print(f"self-test {'ok' if got == want else 'WRONG'}: {name}")
    manifest_cases = (
        ("consistent stale locks", "bbbb", None),
        ("declared family split", "aaaa", "split"),
        ("missing declared patch", "aaaa", "missing"),
    )
    for index, (name, revision, mutation) in enumerate(manifest_cases):
        root = workdir / f"manifest-{index}"
        write_case(root, [("zencodec", a)], [("zencodec", a)], revision)
        manifest = root / "fuzz/Cargo.toml"
        text = manifest.read_text()
        if mutation == "split":
            text = text.replace(
                'zenmeta = { git = "https://github.com/imazen/zencodec", rev = "aaaa" }',
                'zenmeta = { git = "https://github.com/imazen/zencodec", rev = "bbbb" }',
            )
        elif mutation == "missing":
            text = "\n".join(line for line in text.splitlines() if not line.startswith("zenmeta ="))
        manifest.write_text(text)
        problems, _ = check(root)
        ok &= bool(problems)
        print(f"self-test {'ok' if problems else 'WRONG'}: {name}")
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
