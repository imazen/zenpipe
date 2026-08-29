#!/usr/bin/env python3
"""Fail if this repo's AVIF-decoder git deps float, disagree, or sit dead.

WHY THIS EXISTS
---------------
zenpipe decodes AVIF through `zenavif` -> `rav1d-safe`. Neither is on crates.io
at the version we need, so both arrive as git dependencies, and a git dependency
without a `rev` is re-resolved to whatever the default branch points at *at that
moment*. Nothing has to be edited for the decoder underneath this repo to
change, and nothing in any output records that it did.

That is not a theoretical hazard here. Before 2026-08-29 this repo's committed
lockfiles held THREE different zenavif revisions at once -- the root at
`11033c95`, `fuzz/` at `7d950f1c`, and `demo/crate/` fallen all the way back to
the *registry* at 0.1.6 -- and the range they float across contains real decoder
behaviour changes, including the aarch64 NEON conformance campaign of
2026-08-07/08 that took rav1d-safe from 302/766 to 766/766 against dav1d's
published MD5 vectors. AVIF decode is also not exercised by any CI job in this
repo, so nothing else would notice.

WHY IN THIS REPO
----------------
Among the repos that consume this decoder, zenpipe is where a check has the most
to catch and the most to protect:

  * It has the most places to disagree -- several manifests reference
    imazen/zenavif (the root `[patch.crates-io]`, `zencodecs`'s zenavif-parse
    dep line, and the mirrored patch tables in `fuzz/` and `demo/crate/`) across
    several independent `[workspace]`s, each with its own lockfile.
  * It INHERITS the pin rather than owning it, which is exactly the position
    where drift is invisible: zenavif's own manifest decides the rav1d-safe rev,
    so nothing in this repo names the decoder it actually uses.
  * It owns `zencodecs`, which other repos path-dep into (zentone's
    dev/shootout, for one), so a drift here propagates outward.
  * It has live instances of every failure mode below, so the check has teeth on
    day one rather than being aspirational.

Run it against another repo's manifests with `--root`.

THE THREE FAILURE MODES
-----------------------
1. FLOAT      a tracked git dep or patch entry with no `rev`.
2. DISAGREE   a `rev` that differs from the expected rev for that crate.
3. DEAD PATCH a `[patch]` entry that cargo did not use.

(3) is the one that is otherwise invisible, and it is why this checks lockfiles
and not just manifests. A `[patch.crates-io]` can only substitute a package that
something requires FROM THE REGISTRY. Point one at a crate that is reached
through a git dependency instead and cargo silently ignores it: the entry looks
authoritative in the manifest, cargo records it under `[[patch.unused]]` in the
lock, and the graph resolves to something else entirely. That is exactly how a
rav1d-safe patch entry sat inert in zentone's shootout, and how zenmetrics'
patch entry sat dead while its lock resolved a different rev. Cargo prints a
warning, but warnings scroll past; this fails the build.

A patch that is unused merely because the feature that would pull the crate is
turned off is NOT a defect -- `fuzz/` legitimately has one, since it does not
enable AVIF. So a dead patch is only reported when the crate is absent from the
resolved graph AND some manifest in the same workspace declares a dependency
edge that should have brought it in. Anything unused with no such edge is
reported as informational and does not fail.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Crates whose pins must agree, and the rev each must be on.
#
# rav1d-safe is not listed as a manifest-level expectation because no manifest
# in this repo names it: it is selected transitively by whichever zenavif rev is
# pinned. It IS checked in lockfiles, which is where its rev becomes visible.
EXPECTED = {
    "https://github.com/imazen/zenavif": "11033c957df69dcf37a6820032ed2a64f2f6f213",
    "https://github.com/imazen/rav1d-safe": "140f91450c3551c25a5699d12ded6d629ecf6d97",
}

# Packages that come from those repos, for lockfile checking. `zenavif-serialize`
# lives in its own archived repo and is pinned separately, so it is not here.
TRACKED_PACKAGES = {"zenavif", "zenavif-parse", "rav1d-safe"}

SKIP_DIRS = {"target", ".git", ".jj", "node_modules", ".github"}

# `name = { ... git = "URL" ... }`, tolerating any key order and line wrapping.
DEP_RE = re.compile(
    r'^\s*(?P<name>[A-Za-z0-9_-]+)\s*=\s*\{(?P<body>[^}]*)\}', re.MULTILINE
)
GIT_RE = re.compile(r'git\s*=\s*"(?P<url>[^"]+)"')
REV_RE = re.compile(r'rev\s*=\s*"(?P<rev>[0-9a-fA-F]{7,40})"')


def norm_url(url: str) -> str:
    """Compare URLs the way cargo does for our purposes: ignore a `.git` tail."""
    return url.rstrip("/").removesuffix(".git")


def iter_files(root: Path, name: str):
    for path in sorted(root.rglob(name)):
        if any(part in SKIP_DIRS for part in path.relative_to(root).parts):
            continue
        yield path


def check_manifests(root: Path) -> tuple[list[str], int]:
    """FLOAT + DISAGREE, over every Cargo.toml in the tree."""
    problems: list[str] = []
    checked = 0
    for path in iter_files(root, "Cargo.toml"):
        text = path.read_text(encoding="utf-8")
        # Strip comments so a commented-out dep line is not mistaken for a live
        # one -- several manifests here keep disabled deps as comments.
        stripped = "\n".join(
            line.split("#", 1)[0] if not line.lstrip().startswith("#") else ""
            for line in text.splitlines()
        )
        for m in DEP_RE.finditer(stripped):
            git = GIT_RE.search(m.group("body"))
            if not git:
                continue
            url = norm_url(git.group("url"))
            if url not in EXPECTED:
                continue
            checked += 1
            rel = path.relative_to(root)
            name = m.group("name")
            rev = REV_RE.search(m.group("body"))
            if not rev:
                problems.append(
                    f"FLOAT     {rel}: `{name}` -> {url} has no `rev`.\n"
                    f"          It re-resolves to whatever that branch points at, "
                    f"every time, with no edit to any manifest.\n"
                    f"          Add rev = \"{EXPECTED[url]}\"."
                )
            elif not EXPECTED[url].startswith(rev.group("rev").lower()):
                problems.append(
                    f"DISAGREE  {rel}: `{name}` -> {url}\n"
                    f"          pinned {rev.group('rev')}\n"
                    f"          expected {EXPECTED[url]}\n"
                    f"          A rev mismatch on this repo does not just disagree, it "
                    f"SPLITS the graph:\n"
                    f"          cargo treats `git+URL` and `git+URL?rev=X` as different "
                    f"sources, so two\n"
                    f"          copies of the crate land in one build and their error "
                    f"types stop unifying."
                )
    return problems, checked


def lock_is_authoritative(lock: Path, root: Path) -> bool:
    """Is this `Cargo.lock` the one cargo actually uses?

    A workspace MEMBER may carry a leftover `Cargo.lock` from before it joined
    the workspace; cargo ignores it entirely and resolves from the workspace
    root's lock. Reporting drift in a file nothing reads is noise, so those are
    reported as notes instead. A package the root EXCLUDES is its own workspace
    root, so its lock is authoritative even without a `[workspace]` table.
    """
    manifest = lock.parent / "Cargo.toml"
    if lock.parent == root:
        return True
    if manifest.is_file() and re.search(
        r"^\[workspace\]", manifest.read_text(encoding="utf-8"), re.MULTILINE
    ):
        return True
    # Walk up looking for a workspace that claims this directory as a member.
    for parent in lock.parent.parents:
        cand = parent / "Cargo.toml"
        if not cand.is_file():
            if parent == root:
                break
            continue
        text = cand.read_text(encoding="utf-8")
        if not re.search(r"^\[workspace\]", text, re.MULTILINE):
            if parent == root:
                break
            continue
        rel = lock.parent.relative_to(parent).as_posix()
        members = re.search(r"members\s*=\s*\[(.*?)\]", text, re.DOTALL)
        excludes = re.search(r"exclude\s*=\s*\[(.*?)\]", text, re.DOTALL)
        if excludes and f'"{rel}"' in excludes.group(1):
            return True
        if members and f'"{rel}"' in members.group(1):
            return False
        break
    return True


def git_declared(lock_path: Path, root: Path) -> set[str]:
    """Tracked crates that some manifest in this workspace sources from git.

    Only those may be called out for resolving from the registry: a repo that
    simply depends on the published crate is not drifting, it is doing the
    normal thing.
    """
    ws = lock_path.parent
    names: set[str] = set()
    for manifest in ws.rglob("Cargo.toml"):
        if any(part in SKIP_DIRS for part in manifest.relative_to(ws).parts):
            continue
        text = manifest.read_text(encoding="utf-8")
        body = "\n".join(
            line for line in text.splitlines() if not line.lstrip().startswith("#")
        )
        for m in DEP_RE.finditer(body):
            git = GIT_RE.search(m.group("body"))
            if git and norm_url(git.group("url")) in EXPECTED:
                names.add(m.group("name"))
    return names


def split_lock(text: str) -> tuple[str, str]:
    head, sep, tail = text.partition("[[patch.unused]]")
    return head, (sep + tail if sep else "")


def parse_blocks(section: str, marker: str) -> list[dict]:
    out = []
    for block in section.split(marker)[1:]:
        entry = {}
        for key in ("name", "version", "source"):
            m = re.search(rf'^{key} = "([^"]*)"', block, re.MULTILINE)
            if m:
                entry[key] = m.group(1)
        if "name" in entry:
            out.append(entry)
    return out


def workspace_declares(lock_path: Path, root: Path, pkg: str) -> bool:
    """Does any manifest beside this lockfile declare a dep edge on `pkg`?"""
    ws = lock_path.parent
    for manifest in ws.rglob("Cargo.toml"):
        # Relative to the workspace, not absolute: an absolute path may itself
        # contain a `target` or `.git` component (it does under --self-test),
        # which would skip every manifest and silently answer "no".
        if any(part in SKIP_DIRS for part in manifest.relative_to(ws).parts):
            continue
        text = manifest.read_text(encoding="utf-8")
        body = "\n".join(
            line for line in text.splitlines() if not line.lstrip().startswith("#")
        )
        # A dependency edge, not a patch entry: patch tables are what we are
        # testing, so they must not count as evidence that the patch is needed.
        for m in re.finditer(rf'^\s*{re.escape(pkg)}\s*=', body, re.MULTILINE):
            before = body[: m.start()]
            last_table = before.rfind("\n[")
            header = body[last_table : last_table + 40] if last_table >= 0 else ""
            if "patch" not in header:
                return True
    return False


def check_locks(root: Path) -> tuple[list[str], list[str], int]:
    """DEAD PATCH + lockfile-level DISAGREE."""
    problems: list[str] = []
    notes: list[str] = []
    checked = 0
    for path in iter_files(root, "Cargo.lock"):
        rel = path.relative_to(root)
        text = path.read_text(encoding="utf-8")
        if not lock_is_authoritative(path, root):
            if any(f'name = "{p}"' in text for p in TRACKED_PACKAGES):
                notes.append(
                    f"note: {rel}: stale lock beside a WORKSPACE MEMBER -- cargo "
                    f"resolves from the workspace root's lock and never reads this "
                    f"one, so its revs are not checked. Delete it if nothing builds "
                    f"this package standalone."
                )
            continue
        resolved_sec, unused_sec = split_lock(text)
        resolved = parse_blocks(resolved_sec, "[[package]]")
        present = {p["name"] for p in resolved}

        # A third way the decoder can float, distinct from a missing `rev`: a
        # workspace that reaches a tracked crate by PATH into a sibling
        # checkout. Then the rev is whatever that other repo's working tree
        # happens to be on, no manifest here can pin it, and every rev below it
        # is inherited. `zencodecs/fuzz` is deliberately built that way (it
        # path-patches five siblings so it does not have to fetch zenavif's
        # ~600 MB of corpus submodules). Report it, loudly and by name, but do
        # not fail: nothing editable in this repo can change it.
        path_sourced = sorted(
            p["name"]
            for p in resolved
            if p["name"] in TRACKED_PACKAGES and not p.get("source")
        )
        if path_sourced:
            notes.append(
                f"note: {rel}: {', '.join(path_sourced)} resolve by PATH from a "
                f"sibling checkout, so this workspace's decoder rev follows that "
                f"other repo's working tree and cannot be pinned from here. Revs "
                f"below it are inherited and are reported, not enforced."
            )

        for pkg in resolved:
            if pkg["name"] not in TRACKED_PACKAGES:
                continue
            src = pkg.get("source", "")
            if not src.startswith("git+"):
                # A registry source is only a defect if a manifest in this
                # workspace says the crate should come from git. Some repos
                # legitimately consume the published crate -- ravif takes
                # `zenavif-parse = "0.5.2"` straight from crates.io and has no
                # git dep on the zenavif workspace at all.
                if src.startswith("registry+") and pkg["name"] in git_declared(
                    path, root
                ):
                    problems.append(
                        f"DISAGREE  {rel}: `{pkg['name']}` resolves from the REGISTRY at "
                        f"{pkg.get('version', '?')},\n"
                        f"          but a manifest in this workspace declares it from a "
                        f"pinned git source.\n"
                        f"          The lock does not reflect the manifest; re-resolve it "
                        f"(`cargo metadata`)\n"
                        f"          and commit the result."
                    )
                continue
            checked += 1
            url = norm_url(src[len("git+") :].split("?")[0].split("#")[0])
            if url not in EXPECTED:
                continue
            want = EXPECTED[url]
            if f"rev={want}" not in src:
                got = src.split("#")[-1] or "(branch head)"
                line = (
                    f"{rel}: `{pkg['name']}` resolved to {got}\n"
                    f"          expected {want} from {url}"
                )
                if path_sourced:
                    notes.append(
                        f"note: INHERITED {line}\n"
                        f"          (pulled in by the path-sourced crate above; "
                        f"not enforceable from this repo)"
                    )
                else:
                    problems.append(f"DISAGREE  {line}")

        for pkg in parse_blocks(unused_sec, "[[patch.unused]]"):
            if pkg["name"] not in TRACKED_PACKAGES:
                continue
            if pkg["name"] in present:
                problems.append(
                    f"DEAD PATCH {rel}: `{pkg['name']}` is listed under "
                    f"[[patch.unused]] AND resolved\n"
                    f"          from another source. The patch reads as authoritative "
                    f"and controls nothing."
                )
            elif workspace_declares(path, root, pkg["name"]):
                problems.append(
                    f"DEAD PATCH {rel}: `{pkg['name']}` is under [[patch.unused]] but a "
                    f"manifest in this\n"
                    f"          workspace declares a dependency on it. A "
                    f"[patch.crates-io] can only replace a\n"
                    f"          package required FROM THE REGISTRY -- if it is reached "
                    f"through a git dep, cargo\n"
                    f"          ignores the patch silently. Pin it on the dep line "
                    f"instead."
                )
            else:
                notes.append(
                    f"note: {rel}: patch `{pkg['name']}` is unused because nothing in "
                    f"that workspace pulls it in (feature off) -- not a defect."
                )
    return problems, notes, checked


ZENAVIF = "https://github.com/imazen/zenavif"
RAV1D = "https://github.com/imazen/rav1d-safe"
GOOD_AVIF = EXPECTED[ZENAVIF]
GOOD_RAV1D = EXPECTED[RAV1D]

_CLEAN_MANIFEST = f"""\
[package]
name = "probe"
version = "0.0.0"
[workspace]
[patch.crates-io]
zenavif = {{ git = "{ZENAVIF}", rev = "{GOOD_AVIF}" }}
"""

_CLEAN_LOCK = f"""\
version = 4

[[package]]
name = "zenavif"
version = "0.1.7"
source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"

[[package]]
name = "rav1d-safe"
version = "0.6.0"
source = "git+{RAV1D}?rev={GOOD_RAV1D}#{GOOD_RAV1D}"
"""

# (label, manifest, lock, must_fail)
_CASES = [
    ("clean tree", _CLEAN_MANIFEST, _CLEAN_LOCK, False),
    (
        "FLOAT: git dep with no rev",
        _CLEAN_MANIFEST.replace(f', rev = "{GOOD_AVIF}"', ""),
        _CLEAN_LOCK,
        True,
    ),
    (
        "DISAGREE: manifest rev differs from expected",
        _CLEAN_MANIFEST.replace(GOOD_AVIF, "0" * 40),
        _CLEAN_LOCK,
        True,
    ),
    (
        "DISAGREE: lock resolved a different rev than the manifest pins",
        _CLEAN_MANIFEST,
        _CLEAN_LOCK.replace(GOOD_RAV1D, "1" * 40),
        True,
    ),
    (
        "DISAGREE: lock fell back to the registry",
        _CLEAN_MANIFEST,
        _CLEAN_LOCK.replace(
            f'source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"',
            'source = "registry+https://github.com/rust-lang/crates.io-index"',
        ),
        True,
    ),
    (
        # The zenmetrics shape: the patch is declared, a manifest depends on the
        # crate, and cargo used neither -- invisible without reading the lock.
        "DEAD PATCH: declared, depended on, and unused",
        _CLEAN_MANIFEST + f'\n[dependencies]\nzenavif = "0.1.7"\n',
        _CLEAN_LOCK.replace(
            f'[[package]]\nname = "zenavif"\nversion = "0.1.7"\n'
            f'source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"\n',
            "",
        )
        + f'\n[[patch.unused]]\nname = "zenavif"\nversion = "0.1.7"\n'
        f'source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"\n',
        True,
    ),
    (
        # Same lock shape, but nothing depends on the crate: feature simply off.
        "not a defect: patch unused because the feature is off",
        _CLEAN_MANIFEST,
        _CLEAN_LOCK.replace(
            f'[[package]]\nname = "zenavif"\nversion = "0.1.7"\n'
            f'source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"\n',
            "",
        )
        + f'\n[[patch.unused]]\nname = "zenavif"\nversion = "0.1.7"\n'
        f'source = "git+{ZENAVIF}?rev={GOOD_AVIF}#{GOOD_AVIF}"\n',
        False,
    ),
]


def self_test(workdir: Path) -> int:
    """Prove the check fails on each defect, and passes when it should."""
    import io
    import shutil
    from contextlib import redirect_stdout

    workdir.mkdir(parents=True, exist_ok=True)
    failures = 0
    for label, manifest, lock, must_fail in _CASES:
        case = workdir / re.sub(r"[^a-z0-9]+", "-", label.lower())
        shutil.rmtree(case, ignore_errors=True)
        case.mkdir(parents=True)
        (case / "Cargo.toml").write_text(manifest, encoding="utf-8")
        (case / "Cargo.lock").write_text(lock, encoding="utf-8")

        buf = io.StringIO()
        with redirect_stdout(buf):
            man_p, _ = check_manifests(case)
            lock_p, _, _ = check_locks(case)
        failed = bool(man_p + lock_p)

        ok = failed == must_fail
        failures += not ok
        want = "must FAIL" if must_fail else "must PASS"
        print(f"  [{'ok  ' if ok else 'BAD '}] {want}: {label}")
        if not ok:
            for p in man_p + lock_p:
                print("      " + p.replace("\n", "\n      "))
        elif must_fail:
            first = (man_p + lock_p)[0].splitlines()[0]
            print(f"           -> {first.strip()}")

    shutil.rmtree(workdir, ignore_errors=True)
    if failures:
        print(f"\nself-test FAILED: {failures} case(s) behaved wrongly")
        return 1
    print("\nself-test OK -- the check fires on float, disagreement and dead patch,")
    print("and stays quiet on a clean tree and on a legitimately-unused patch.")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repo root to audit (default: this repo)",
    )
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="verify the check itself fires on each failure mode, then exit",
    )
    ap.add_argument(
        "--expect",
        action="append",
        default=[],
        metavar="URL=REV",
        help="override the expected rev for one repo (repeatable). Needed when "
        "auditing another repo with --root: repos may pin different revs on "
        "purpose, and running without this reports the cross-repo drift.",
    )
    args = ap.parse_args()
    root = args.root.resolve()

    for spec in args.expect:
        url, _, rev = spec.partition("=")
        if not re.fullmatch(r"[0-9a-fA-F]{7,40}", rev):
            print(f"--expect wants URL=REV with a hex rev, got: {spec}", file=sys.stderr)
            return 2
        EXPECTED[norm_url(url)] = rev.lower()

    if args.self_test:
        print("check-decoder-pins self-test:")
        return self_test(root / "target" / "check-decoder-pins-selftest")

    man_problems, man_n = check_manifests(root)
    lock_problems, notes, lock_n = check_locks(root)
    problems = man_problems + lock_problems

    print(f"check-decoder-pins: {root}")
    for url, rev in sorted(EXPECTED.items()):
        print(f"  expected  {url} @ {rev}")
    print(f"  scanned   {man_n} manifest entries, {lock_n} lockfile entries")
    for n in notes:
        print(f"  {n}")

    if problems:
        print(f"\nFAILED -- {len(problems)} problem(s):\n")
        for p in problems:
            print(p + "\n")
        print(
            "If a rev SHOULD move, move it in every manifest at once and re-resolve\n"
            "every lockfile, then update EXPECTED in this script in the same commit."
        )
        return 1

    print("\nOK -- every tracked decoder dep is pinned, agrees, and is live.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
