#!/usr/bin/env python3
"""Serial fresh-target builds; git archives, no worktrees. Fetch is untimed.
Compare the integrated parent against the scrubber with identical feature sets,
then explicitly enable metadata. JSON records every run and normal dependency
count/depth. No compiler cache or incremental compilation is used.
"""
import argparse, io, json, os, pathlib, platform, statistics, subprocess, tarfile, time
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--before', required=True)
p.add_argument('--after', required=True)
p.add_argument('--work', type=pathlib.Path, required=True)
p.add_argument('--repeats', type=int, default=3)
p.add_argument('--only', nargs='*')
a = p.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
a.work.mkdir(parents=True, exist_ok=True)
for name, rev in [('before', a.before), ('after', a.after)]:
    dst = a.work / name
    dst.mkdir(exist_ok=True)
    archive = subprocess.check_output(['git', 'archive', rev], cwd=root)
    with tarfile.open(fileobj=io.BytesIO(archive)) as f:
        f.extractall(dst, filter='data')
features = {'minimal': '', 'jpeg-hdr': 'jpeg-ultrahdr', 'jxl': 'jxl-encode,jxl-decode', 'png': 'png'}
cases = {}
for name, feature in features.items():
    for version in ['before', 'after-off', 'after-on']:
        cases[name+'-'+version] = (version.split('-')[0], ','.join(filter(None, [feature, 'metadata' if version == 'after-on' else ''])))
cases['runtime-services'] = ('after', None)
if a.only: cases = {k:v for k,v in cases.items() if k in a.only}
report = {'before': a.before, 'after': a.after, 'rustc': subprocess.check_output(['rustc','-Vv'], text=True), 'platform': platform.platform(), 'cpu': pathlib.Path('/proc/cpuinfo').read_text().split('model name')[1].split('\n')[0], 'jobs': 4, 'cases': {}}
for name, (version, fs) in cases.items():
    cwd = a.work / version
    args = ['-p','zencodecs','--no-default-features'] + (['--features',fs] if fs else [])
    if fs is None:
        cwd = cwd/'tools/metadata-services'
        args = []
    with (a.work/(name+'-fetch.log')).open('w') as log:
        subprocess.run(['cargo','fetch'],cwd=cwd,stdout=log,stderr=log,check=True)
    tree = subprocess.check_output(['cargo','tree','--offline','--edges','normal','--prefix','depth','--no-dedupe',*args],cwd=cwd,text=True,stderr=subprocess.DEVNULL)
    lines = [line for line in tree.splitlines() if line and line[0].isdigit()]
    packages = set(line.lstrip('0123456789') for line in lines)
    depth = max((int(line[:len(line)-len(line.lstrip('0123456789'))]) for line in lines),default=0)
    result = {'features':fs,'normal_packages_including_root':len(packages),'normal_max_edges':depth,'cold_seconds':[],'warm_seconds':[]}
    report['cases'][name] = result
    for repeat in range(a.repeats):
        target = a.work/(name+f'-target-{repeat}')
        env = dict(os.environ,CARGO_TARGET_DIR=str(target),CARGO_INCREMENTAL='0',RUSTC_WRAPPER='',RUSTC_WORKSPACE_WRAPPER='')
        for stage in ['cold','warm']:
            with (a.work/(name+f'-{repeat}-{stage}.log')).open('w') as log:
                start = time.perf_counter()
                subprocess.run(['cargo','build','--lib','--offline','-j4',*args],cwd=cwd,env=env,stdout=log,stderr=log,check=True)
                result[stage+'_seconds'].append(round(time.perf_counter()-start,3))
        (a.work/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    result['cold_median_seconds'] = statistics.median(result['cold_seconds'])
    (a.work/'results.json').write_text(json.dumps(report,indent=2)+'\n')
    print(name, json.dumps(result), flush=True)
