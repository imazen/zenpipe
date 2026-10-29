#!/usr/bin/env python3
"""Classify a strict-Clippy log against a known-lint baseline, failing closed.

  ic_compare.py LOG BASELINE RC OUT

Exit 0 only when the run is either clean (rc 0, no diagnostics) or fails on
known baseline lints alone. Everything else exits 1 and says why:
- an exit status other than 0 or 101 (a timeout is 124, a crash or a missing
  tool is anything else);
- a diagnostic without a source location, or with a compiler error code
  (`error[E0425]`): those are compiler or infrastructure failures, not lints;
- a missing or wrong `could not compile ... due to N previous errors` summary,
  or one whose N differs from the located diagnostics parsed;
- any located lint that is not in the baseline.
Baseline-only is still a strict-Clippy FAILURE; the caller reports it so.
"""
import re
import sys

HEADER = re.compile(r"^(error|warning)(\[[A-Za-z0-9_]+\])?: (.*)$")
SUMMARY = re.compile(r"^error: could not compile `[^`]+` \(lib\) due to (\d+) previous errors?(; .*)?$")
LOCATION = re.compile(r"^\s+--> (\S+)")


def classify(lines, baseline, rc):
    problems = []
    if rc not in (0, 101):
        problems.append(f"tool exit status {rc} (only 0 and 101 are lint outcomes)")
    heads = [i for i, line in enumerate(lines) if HEADER.match(line)]
    located, summaries = [], []
    for n, i in enumerate(heads):
        line = lines[i]
        end = heads[n + 1] if n + 1 < len(heads) else len(lines)
        summary = SUMMARY.match(line)
        if summary:
            summaries.append(int(summary.group(1)))
            continue
        code = HEADER.match(line).group(2)
        where = next((LOCATION.match(lines[j]).group(1) for j in range(i + 1, end) if LOCATION.match(lines[j])), None)
        if code:
            problems.append(f"compiler error {code}: {line}")
        elif where is None:
            problems.append(f"unlocated diagnostic: {line}")
        else:
            located.append(f"{where}\t{HEADER.match(line).group(3)}")
    if rc == 0:
        if located or summaries:
            problems.append("exit 0 but diagnostics were reported")
        verdict = "clean"
    else:
        if len(summaries) != 1:
            problems.append(f"{len(summaries)} could-not-compile summaries, expected 1")
        elif summaries[0] != len(located):
            problems.append(f"summary counts {summaries[0]} errors, {len(located)} located lints parsed")
        if not located:
            problems.append("failure without any parsed lint")
        verdict = "baseline-only"
    new = sorted(set(located) - baseline)
    problems += [f"new lint: {k}" for k in new]
    gone = sorted(baseline - set(located)) if rc != 0 else []
    return ("FAIL" if problems else verdict), located, new, gone, problems


def main():
    log_path, baseline_path, rc, out_path = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
    lines = re.sub(r"\x1b\[[0-9;]*m", "", open(log_path, errors="replace").read()).splitlines()
    baseline = {k for k in open(baseline_path).read().split("\n") if k}
    verdict, located, new, gone, problems = classify(lines, baseline, rc)
    with open(out_path, "w") as f:
        f.write(f"verdict {verdict} rc {rc} lints {len(located)} baseline {len(baseline)} new {len(new)} no-longer-reported {len(gone)}\n")
        f.writelines(f"PROBLEM\t{p}\n" for p in problems)
        f.writelines(f"GONE\t{k}\n" for k in gone)
    print(f"verdict={verdict} rc={rc} lints={len(located)} new={len(new)} gone={len(gone)} problems={len(problems)}")
    for p in problems[:10]:
        print(f"  PROBLEM {p}")
    sys.exit(0 if verdict in ("clean", "baseline-only") else 1)


if __name__ == "__main__":
    main()
