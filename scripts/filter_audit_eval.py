#!/usr/bin/env python3
"""Evaluate filter audit montages via OpenAI Vision API.

Reads manifest.json from the audit output directory, sends each montage
to gpt-4.1-nano for analysis, and generates a severity report.

Usage:
    python3 scripts/filter_audit_eval.py [audit_dir]

    audit_dir defaults to /mnt/v/output/zenfilters/audit/

Requires: OPENAI_API_KEY environment variable.
"""

import json
import sys
import time
import base64
from pathlib import Path
from openai import OpenAI

MODEL = "gpt-4.1-mini"
AUDIT_DIR = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("/mnt/v/output/zenfilters/audit")
RESULTS_FILE = AUDIT_DIR / "audit_results.json"
REPORT_FILE = AUDIT_DIR / "audit_report.md"

# Rate limiting
MAX_REQUESTS_PER_SECOND = 40
MIN_INTERVAL = 1.0 / MAX_REQUESTS_PER_SECOND


def load_existing_results():
    """Load previously completed results for resumability."""
    if RESULTS_FILE.exists():
        with open(RESULTS_FILE) as f:
            return json.load(f)
    return []


def save_results(results):
    """Save results atomically."""
    tmp = RESULTS_FILE.with_suffix(".tmp")
    with open(tmp, "w") as f:
        json.dump(results, f, indent=2)
    tmp.rename(RESULTS_FILE)


def encode_image(path):
    """Read and base64-encode a JPEG image."""
    with open(path, "rb") as f:
        return base64.b64encode(f.read()).decode("utf-8")


def evaluate_montage(client, entry, image_path):
    """Send a single montage to OpenAI Vision and parse the response."""
    b64 = encode_image(image_path)

    prompt = (
        f'Compare these two halves of the image. Left is the original photo, '
        f'right has the "{entry["filter"]}" filter applied at {entry["level"]} '
        f'intensity ({entry["params"]}).\n\n'
        f'Answer these 4 questions as JSON (no markdown, just raw JSON):\n'
        f'{{\n'
        f'  "effect": "yes|barely|no",\n'
        f'  "banding": "yes|no",\n'
        f'  "artifacts": "none or description",\n'
        f'  "quality": "good|questionable|broken"\n'
        f'}}'
    )

    response = client.chat.completions.create(
        model=MODEL,
        messages=[
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": f"data:image/jpeg;base64,{b64}",
                            "detail": "low",
                        },
                    },
                ],
            }
        ],
        max_tokens=200,
    )

    raw = response.choices[0].message.content.strip()

    # Parse JSON from response (handle markdown code blocks)
    text = raw
    if text.startswith("```"):
        lines = text.split("\n")
        text = "\n".join(lines[1:-1] if lines[-1].strip() == "```" else lines[1:])

    try:
        result = json.loads(text)
    except json.JSONDecodeError:
        result = {"effect": "?", "banding": "?", "artifacts": raw, "quality": "?"}

    return result


def generate_report(results):
    """Generate a markdown severity report from results."""
    # Group by filter
    by_filter = {}
    for r in results:
        name = r["filter"]
        if name not in by_filter:
            by_filter[name] = []
        by_filter[name].append(r)

    broken = []
    no_effect = []
    questionable = []
    ok = []

    for name, entries in sorted(by_filter.items()):
        has_banding = any(e.get("banding") == "yes" for e in entries)
        has_broken = any(e.get("quality") == "broken" for e in entries)
        # "no effect" at moderate or extreme is a bug
        no_effect_at_strong = any(
            e.get("effect") == "no" and e.get("level") in ("moderate", "extreme")
            for e in entries
        )
        has_artifacts = any(
            e.get("artifacts") not in ("none", "None", None, "")
            and e.get("artifacts") != "?"
            for e in entries
        )
        has_questionable = any(e.get("quality") == "questionable" for e in entries)

        summary = {
            "filter": name,
            "entries": entries,
            "banding": has_banding,
            "broken_quality": has_broken,
            "no_effect": no_effect_at_strong,
            "has_artifacts": has_artifacts,
        }

        if has_banding or has_broken:
            broken.append(summary)
        elif no_effect_at_strong:
            no_effect.append(summary)
        elif has_questionable or has_artifacts:
            questionable.append(summary)
        else:
            ok.append(summary)

    lines = ["# Filter Audit Report\n"]
    lines.append(f"**Total filters tested:** {len(by_filter)}")
    lines.append(f"**Total montages evaluated:** {len(results)}")
    lines.append(f"**Model:** {MODEL}\n")

    if broken:
        lines.append(f"## BROKEN ({len(broken)} filters)\n")
        for s in broken:
            lines.append(f"### {s['filter']}")
            for e in s["entries"]:
                flag = ""
                if e.get("banding") == "yes":
                    flag += " **BANDING**"
                if e.get("quality") == "broken":
                    flag += " **BROKEN**"
                artifacts = e.get("artifacts", "none")
                lines.append(
                    f"- `{e['level']}` ({e['params']}): "
                    f"effect={e.get('effect')}, quality={e.get('quality')}, "
                    f"artifacts={artifacts}{flag}"
                )
            lines.append("")

    if no_effect:
        lines.append(f"## NO EFFECT ({len(no_effect)} filters)\n")
        for s in no_effect:
            lines.append(f"### {s['filter']}")
            for e in s["entries"]:
                if e.get("effect") == "no":
                    lines.append(
                        f"- `{e['level']}` ({e['params']}): **NO VISIBLE EFFECT**"
                    )
            lines.append("")

    if questionable:
        lines.append(f"## QUESTIONABLE ({len(questionable)} filters)\n")
        for s in questionable:
            lines.append(f"### {s['filter']}")
            for e in s["entries"]:
                artifacts = e.get("artifacts", "none")
                if artifacts not in ("none", "None", None, ""):
                    lines.append(
                        f"- `{e['level']}` ({e['params']}): "
                        f"quality={e.get('quality')}, artifacts={artifacts}"
                    )
            lines.append("")

    if ok:
        lines.append(f"## OK ({len(ok)} filters)\n")
        for s in ok:
            lines.append(f"- {s['filter']}")
        lines.append("")

    report = "\n".join(lines)
    with open(REPORT_FILE, "w") as f:
        f.write(report)
    return report


def main():
    manifest_path = AUDIT_DIR / "manifest.json"
    if not manifest_path.exists():
        print(f"No manifest.json found in {AUDIT_DIR}")
        print("Run: cargo run --release --features experimental --example filter_audit")
        sys.exit(1)

    with open(manifest_path) as f:
        manifest = json.load(f)

    print(f"Loaded {len(manifest)} entries from manifest")

    # Load existing results for resumability
    existing = load_existing_results()
    done_keys = {(r["filter"], r["level"], r["source"]) for r in existing}
    print(f"Already completed: {len(done_keys)} entries")

    todo = [
        e for e in manifest
        if (e["filter"], e["level"], e["source"]) not in done_keys
    ]
    print(f"Remaining: {len(todo)} entries")

    if not todo:
        print("All entries already evaluated. Generating report...")
        report = generate_report(existing)
        print(f"\nReport written to {REPORT_FILE}")
        print(report)
        return

    client = OpenAI()
    results = list(existing)
    last_request_time = 0.0
    errors = 0

    for i, entry in enumerate(todo):
        image_path = AUDIT_DIR / entry["image"]
        if not image_path.exists():
            print(f"  SKIP (missing): {entry['image']}")
            continue

        # Rate limiting
        elapsed = time.time() - last_request_time
        if elapsed < MIN_INTERVAL:
            time.sleep(MIN_INTERVAL - elapsed)

        try:
            result = evaluate_montage(client, entry, image_path)
            last_request_time = time.time()
        except Exception as e:
            errors += 1
            print(f"  ERROR ({errors}): {e}")
            if errors > 10:
                print("Too many errors, stopping.")
                break
            # Back off on error
            time.sleep(2.0)
            continue

        entry_result = {
            **entry,
            **result,
        }
        results.append(entry_result)

        # Save progress every 10 entries
        if (i + 1) % 10 == 0:
            save_results(results)

        status = f"effect={result.get('effect', '?')}"
        if result.get("banding") == "yes":
            status += " BANDING!"
        if result.get("quality") == "broken":
            status += " BROKEN!"
        if result.get("effect") == "no":
            status += " NO_EFFECT!"

        print(f"  [{i+1}/{len(todo)}] {entry['filter']}/{entry['level']}/{entry['source']}: {status}")

    # Final save
    save_results(results)
    print(f"\nResults saved to {RESULTS_FILE}")

    # Generate report
    report = generate_report(results)
    print(f"Report written to {REPORT_FILE}")
    print("\n" + report)


if __name__ == "__main__":
    main()
