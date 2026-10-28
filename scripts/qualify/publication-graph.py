#!/usr/bin/env python3
"""Record six resolved graphs and fail on unexpected source families or drift.

This supplements the existing five-lock core/decoder gates. The expectation
file identifies a frozen experiment; it is not an alternate Cargo source map.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys
import tomllib

LOCKS = ('Cargo.lock', 'fuzz/Cargo.lock', 'demo/crate/Cargo.lock',
         'tools/metadata-services/Cargo.lock', 'zencodecs/fuzz/Cargo.lock',
         'apidoc/Cargo.lock')
CORE = {'zencodec', 'zencodec-io', 'zencodec-coroutine', 'zencodec-testkit',
        'zencodec-media', 'zencodec-media-testkit', 'zenmeta'}
PIXELS = {'zenpixels', 'zenpixels-convert'}
EXTRAS = {'zentiff', 'zenpdf', 'zensvg', 'zenjp2', 'zenexr', 'zenpsd', 'zenivf', 'zenwebm'}


def identity(package):
    return (package['name'], package['version'], package.get('source', 'path'),
            package.get('checksum', ''))


def inspect(root, expected, before, results):
    errors, graphs, deltas, metadata = [], {}, {}, {}
    reviewed, admitted = Counter(), Counter()
    if expected.get('reviewed_deltas_file'):
        ledger = json.loads((root / expected['reviewed_deltas_file']).read_text())
        valid = ledger['schema'] == 1
        for name, want in ledger['before_lock_sha256'].items():
            if hashlib.sha256((before / name).read_bytes()).hexdigest() != want:
                errors.append(f'{name}: reviewed delta baseline mismatch')
                valid = False
        for name, want in ledger['scope_sources'].items():
            if expected.get(name) != want:
                errors.append(f'{name}: reviewed delta source scope mismatch')
                valid = False
        if valid:
            reviewed.update((r['lock'], r['change'], tuple(r['identity']))
                            for r in ledger['records'])
    allowed = CORE | PIXELS | EXTRAS | {'zenavif', 'zenavif-parse', 'rav1d-safe',
                                      'tiff', 'hayro-syntax'}
    sources = {**{name: expected['core_source'] for name in CORE},
               **{name: expected['pixel_source'] for name in PIXELS},
               'zenavif': expected['avif_source'],
               'zenavif-parse': expected['avif_source'],
               'rav1d-safe': expected['rav1d_source']}
    for relative in LOCKS:
        path = root / relative
        old = tomllib.loads((before / relative).read_text())['package']
        packages = tomllib.loads(path.read_text())['package']
        old_ids, new_ids = Counter(map(identity, old)), Counter(map(identity, packages))
        added, removed = list((new_ids-old_ids).elements()), list((old_ids-new_ids).elements())
        deltas[relative] = {'added': added, 'removed': removed}
        for change, items in [('added', added), ('removed', removed)]:
            for item in items:
                if item[0] not in allowed:
                    record = (relative, change, item)
                    if admitted[record] < reviewed[record]:
                        admitted[record] += 1
                    else:
                        errors.append(f'{relative}: unreviewed package drift: {item}')
        tracked = {}
        for package in packages:
            name, version, source, _ = identity(package)
            if name in sources or name in EXTRAS or name == 'how-far':
                tracked.setdefault(name, []).append(identity(package))
                if name in sources and source != sources[name]:
                    errors.append(f'{relative}: wrong source: {identity(package)}')
                if name in EXTRAS and source != expected.get('extras_source', 'path'):
                    errors.append(f'{relative}: wrong frozen extras source: {identity(package)}')
                if name == 'how-far' and (version != '0.1.0' or not source.startswith('registry+')):
                    errors.append(f'{relative}: unexpected how-far: {identity(package)}')
            if name == 'tiff' and version == '0.11.3' and source != expected['tiff_source']:
                errors.append(f'{relative}: tiff 0.11.3 missed the qualified fork: {source}')
            if name == 'hayro-syntax' and source != expected['hayro_source']:
                errors.append(f'{relative}: hayro-syntax missed the qualified fork: {source}')
        for name, copies in tracked.items():
            if len(copies) != 1:
                errors.append(f'{relative}: duplicate {name}: {copies}')
            # Retain exact versions for all pre-existing packages except the
            # explicitly changed AV1 decoder (0.6 -> 0.7).
            previous = [p['version'] for p in old if p['name'] == name]
            if previous and name != 'rav1d-safe' and [c[1] for c in copies] != previous:
                errors.append(f'{relative}: unreviewed version change for {name}: {previous} -> {copies}')
        missing = ({p['name'] for p in old} & (CORE | PIXELS | EXTRAS | {'zenavif', 'zenavif-parse', 'rav1d-safe', 'how-far'})) - set(tracked)
        if missing:
            errors.append(f'{relative}: existing tracked packages disappeared: {sorted(missing)}')
        graphs[relative] = {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'packages': tracked}
        log = results / ('metadata-' + relative.replace('Cargo.lock', 'Cargo.toml').replace('/', '-') + '.json')
        try:
            document = json.loads(log.read_text())
            metadata[relative] = {'packages': [{'id': p['id'], 'manifest_path': p['manifest_path']} for p in document['packages']], 'resolve': document['resolve']}
            for p in document['packages']:
                if p['name'] in EXTRAS:
                    if 'extras_source' in expected:
                        if p.get('source') != expected['extras_source']:
                            errors.append(f'{relative}: wrong extras metadata source: {p["id"]}')
                    else:
                        want = Path(expected['extras_export']) / p['name'] / 'Cargo.toml'
                        if Path(p['manifest_path']).resolve() != want.resolve():
                            errors.append(f'{relative}: wrong extras export: {p["manifest_path"]}')
        except (OSError, ValueError, KeyError) as error:
            errors.append(f'{relative}: missing/invalid executed metadata: {error}')
    full = results / 'metadata-full-api.json'
    try:
        document = json.loads(full.read_text())
        metadata['full-api'] = document
        codecs = next(p for p in document['packages'] if p['name'] == 'zencodecs')
        node = next(n for n in document['resolve']['nodes'] if n['id'] == codecs['id'])
        required = {'all', 'jp2-decode', 'metadata', 'checked-transcode-async', 'heic-decode', 'raw-decode'}
        if not required.issubset(node['features']):
            errors.append(f'full API profile lost features: {sorted(required-set(node["features"]))}')
    except (OSError, ValueError, KeyError, StopIteration) as error:
        errors.append(f'missing/invalid full API metadata: {error}')
    if reviewed != admitted:
        errors.append('reviewed delta records do not exactly match this migration')
    return {'graphs': graphs, 'package_deltas': deltas, 'metadata': metadata,
            'reviewed_delta_records': sum(admitted.values()), 'errors': errors}


if __name__ == '__main__':
    root, expected, before, results = map(Path, sys.argv[1:])
    report = inspect(root, json.loads(expected.read_text()), before, results)
    # Never replace an earlier evidence capture, even when the audit fails.
    index = 0
    while (results / f'graph-identities-{index}.json').exists():
        index += 1
    destination = results / f'graph-identities-{index}.json'
    destination.write_text(json.dumps(report, indent=2) + '\n')
    print(f'graph report: {destination}')
    for error in report['errors']:
        print('FAIL:', error)
    print(f'{len(report["graphs"])}/6 lock graphs recorded; {len(report["errors"])} error(s)')
    sys.exit(bool(report['errors']))
