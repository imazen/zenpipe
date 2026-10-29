"""Source-only audit controls; pass the retained frozen run as the argument."""
from pathlib import Path
import copy,importlib.util,json,tempfile,sys
sys.dont_write_bytecode=True
root=Path(__file__).resolve().parents[2];frozen=Path(sys.argv[1]).resolve()
spec=importlib.util.spec_from_file_location('audit',root/'scripts/qualify/publication-graph.py');audit=importlib.util.module_from_spec(spec);spec.loader.exec_module(audit)
original=json.loads((frozen/'EXPECTED.json').read_text());ledger_path=root/'scripts/qualify/publication-deltas-2026-10-28.json';ledger=json.loads(ledger_path.read_text())
def inspect(expected):return audit.inspect(frozen/'zenpipe',expected,frozen/'results/before',frozen/'results')
a=inspect(original);assert len(a['errors'])==52;print('PASS existing gate still refuses all52 without explicit ledger')
expected={**original,'reviewed_deltas_file':str(ledger_path)};a=inspect(expected);assert a['errors']==[],a['errors'];assert a['reviewed_delta_records']==52;print('PASS all52 exact source-reviewed records admitted on unchanged frozen receipts')
with tempfile.TemporaryDirectory(prefix='publication-ledger-control-',dir=Path.home()/'tmp') as tmp:
 p=Path(tmp)/'ledger.json'
 def change(config):p.write_text(json.dumps(config));return inspect({**original,'reviewed_deltas_file':str(p)})
 altered=copy.deepcopy(ledger);altered['records'][0]['identity'][3]='wrong-checksum';a=change(altered);assert any('unreviewed package drift' in e for e in a['errors']);print('PASS changed checksum refused')
 altered=copy.deepcopy(ledger);altered['records'][0]['identity'][1]='99.0.0';a=change(altered);assert any('unreviewed package drift' in e for e in a['errors']);print('PASS changed version refused')
 altered=copy.deepcopy(ledger);altered['records'][0]['change']='removed';a=change(altered);assert a['errors'];print('PASS changed delta direction refused')
 altered=copy.deepcopy(ledger);altered['records'][0]['lock']='demo/crate/Cargo.lock';a=change(altered);assert a['errors'];print('PASS changed graph scope refused')
 altered=copy.deepcopy(ledger);altered['before_lock_sha256']['Cargo.lock']='wrong';a=change(altered);assert any('baseline mismatch' in e for e in a['errors']);assert a['reviewed_delta_records']==0;print('PASS different baseline refused')
 altered=copy.deepcopy(ledger);altered['scope_sources']['extras_source']='git+unreviewed';a=change(altered);assert any('source scope mismatch' in e for e in a['errors']);assert a['reviewed_delta_records']==0;print('PASS different immutable pin scope refused')
 altered=copy.deepcopy(ledger);altered['records'].append(altered['records'][0]);a=change(altered);assert any('exactly match' in e for e in a['errors']);print('PASS unused extra/duplicate admission refused')
print('9/9 exact ledger Python controls; only retained metadata/locks read, no Cargo')
