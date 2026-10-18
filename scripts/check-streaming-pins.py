#!/usr/bin/env python3
"""Check the committed streaming interface graph without Cargo source overrides."""

import argparse
import copy
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
MANIFESTS = (
    "Cargo.toml",
    "fuzz/Cargo.toml",
    "zencodecs/fuzz/Cargo.toml",
    "demo/crate/Cargo.toml",
    "wasm-size-shim/Cargo.toml",
)
INTERFACE_PACKAGES = ("zencodec", "zencodec-io", "zencodec-testkit", "zenmeta")
URL = "https://github.com/imazen/zencodec"


def check_graph(manifest, lock, expected):
    errors = []
    pin = manifest.get("patch", {}).get("crates-io", {}).get("zencodec", {})
    if pin.get("git") != URL or pin.get("rev") != expected or "path" in pin:
        errors.append("zencodec patch must use the production git revision")
    for name in INTERFACE_PACKAGES:
        companion = manifest.get("patch", {}).get("crates-io", {}).get(name)
        if companion is not None and (companion.get("git") != URL or companion.get("rev") != expected or "path" in companion):
            errors.append(f"{name} patch differs from production")
        packages = [p for p in lock["package"] if p["name"] == name]
        if name == "zencodec" and len(packages) != 1:
            errors.append("lock must contain exactly one zencodec package")
        if len(packages) > 1:
            errors.append(f"lock contains duplicate {name} packages")
        for package in packages:
            source = package.get("source", "")
            if not source.startswith(f"git+{URL}?") or not source.endswith(f"#{expected}"):
                errors.append(f"{name} lock source differs from production")
    return errors


def self_test():
    rev = "1" * 40
    manifest = {"patch": {"crates-io": {"zencodec": {"git": URL, "rev": rev}}}}
    lock = {"package": [{"name": "zencodec", "source": f"git+{URL}?rev={rev}#{rev}"}]}
    assert not check_graph(manifest, lock, rev)
    bad = copy.deepcopy(manifest)
    bad["patch"]["crates-io"]["zencodec"] = {"path": "../../zencodec"}
    assert check_graph(bad, lock, rev)
    bad = copy.deepcopy(manifest)
    bad["patch"]["crates-io"]["zencodec"]["rev"] = "2" * 40
    assert check_graph(bad, lock, rev)
    for source in (None, "registry+https://github.com/rust-lang/crates.io-index", f"git+{URL}?rev=old#old"):
        bad = copy.deepcopy(lock)
        if source is None:
            del bad["package"][0]["source"]
        else:
            bad["package"][0]["source"] = source
        assert check_graph(manifest, bad, rev)
    bad = copy.deepcopy(lock)
    bad["package"].append(copy.deepcopy(bad["package"][0]))
    assert check_graph(manifest, bad, rev)
    for name in INTERFACE_PACKAGES[1:]:
        stale = copy.deepcopy(manifest)
        stale["patch"]["crates-io"][name] = {"git": URL, "rev": "2" * 40}
        assert check_graph(stale, lock, rev)
        bad = copy.deepcopy(lock)
        bad["package"].append({"name": name, "source": f"git+{URL}?rev=old#old"})
        assert check_graph(manifest, bad, rev)
    print("streaming pin negative controls pass")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    if parser.parse_args().self_test:
        self_test()
        return 0
    production = tomllib.loads((ROOT / MANIFESTS[0]).read_text())
    expected = production["patch"]["crates-io"]["zencodec"]["rev"]
    if len(expected) != 40 or any(c not in "0123456789abcdef" for c in expected):
        raise ValueError("production zencodec revision must be a full commit hash")
    failures = []
    for relative in MANIFESTS:
        path = ROOT / relative
        manifest = tomllib.loads(path.read_text())
        lock = tomllib.loads(path.with_name("Cargo.lock").read_text())
        errors = check_graph(manifest, lock, expected)
        for table in manifest.get("patch", {}).values():
            for name, patch in table.items():
                if "path" in patch and not (path.parent / patch["path"]).resolve().is_relative_to(ROOT):
                    errors.append(f"external sibling patch: {name}")
        failures.extend(f"{relative}: {error}" for error in errors)
        if not errors:
            print(f"{relative}: one zencodec at {expected}")
    for failure in failures:
        print(failure, file=sys.stderr)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
