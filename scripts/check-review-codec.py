#!/usr/bin/env python3
"""Validate the pinned transcode adapter against an explicit local codec review.

Run only in an exported workspace: repository manifests and locks are untouched.
Both registry edges and direct git edges must resolve to the same review tree.
"""
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

root = Path.cwd()
if (root / '.git').exists() or (root / '.jj').exists():
    raise SystemExit('Run in a disposable source export, not a repository')
codec = Path(sys.argv[1]).resolve(strict=True)
manifest = root / 'Cargo.toml'
lock = root / 'Cargo.lock'
original_manifest = manifest.read_bytes()
original_lock = lock.read_bytes()
config = root / '.cargo' / 'config.toml'
original_config = config.read_bytes() if config.exists() else None
try:
    text, count = re.subn(
        r'^zencodec = \{ git = "https://github.com/imazen/zencodec", rev = "[a-f0-9]+" \}$',
        'zencodec = { path = ' + json.dumps(str(codec)) + ' }',
        original_manifest.decode(), flags=re.MULTILINE,
    )
    if count != 1:
        raise RuntimeError(f'Expected one registry patch, found {count}')
    manifest.write_text(text)
    config.parent.mkdir(exist_ok=True)
    entries = ['[patch."https://github.com/imazen/zencodec"]']
    for package in ('zencodec', 'zencodec-io', 'zencodec-coroutine', 'zenmeta'):
        path = codec if package == 'zencodec' else codec / package
        entries.append(package + ' = { path = ' + json.dumps(str(path)) + ' }')
    config.write_bytes((original_config or b'') + ('\n' + '\n'.join(entries) + '\n').encode())
    cargo = ['cargo']
    subprocess.run(cargo + ['update', '-p', 'zencodecs'], check=True)
    resolved = [p for p in tomllib.loads(lock.read_text())['package'] if p['name'] == 'zencodec']
    if len(resolved) != 1 or 'source' in resolved[0]:
        raise RuntimeError(f'Review graph must contain exactly one local zencodec: {resolved}')
    args = ['-p', 'zencodecs', '--no-default-features', '--features',
            'std,cms,jpeg,png,checked-transcode-async', '--locked']
    subprocess.run(cargo + ['test'] + args + ['--test', 'checked_transcode'], check=True)
    subprocess.run(cargo + ['clippy'] + args + ['--lib', '--test', 'checked_transcode',
                   '--example', 'checked_transcode', '--', '-D', 'warnings'], check=True)
finally:
    manifest.write_bytes(original_manifest)
    lock.write_bytes(original_lock)
    if original_config is None:
        config.unlink(missing_ok=True)
    else:
        config.write_bytes(original_config)
