#!/usr/bin/env bash
# Memory matrix for zenpipe::io output schedules (tools/io-check examples).
#
#   memory_matrix.sh measure PROBE OUT_DIR [SIZE]
#   memory_matrix.sh controls PROBE ORACLE OUT_DIR [SIZE]
#
# PROBE is a release build of the memory_probe example (record its sha256 and
# source commit; reuse the same binary for the controls). ORACLE is the
# png_oracle example. measure: authors a SIZE x SIZE RGB8 PNG fixture
# (default 2048), then runs PNG effort 0 and 2 x oneshot/live/memory/disk, each
# cell once under /usr/bin/time -v (max RSS) and once under heaptrack (peak
# heap), writing OUT_DIR/peaks.tsv. Outputs are deleted after measuring.
# controls: reruns every cell untimed, keeps each output and checks it with the
# independent png_oracle against the authored samples (OUT_DIR/controls.tsv).
# A refusal is recorded as a refusal, never as a measured route. Numbers are
# whole-process, at this size only; wall times are not throughput results.
set -u
mode=$1
case "$mode" in
measure) PROBE=$2; OUT=$3; SIZE=${4:-2048} ;;
controls) PROBE=$2; ORACLE=$3; OUT=$4; SIZE=${5:-2048} ;;
*) echo "usage: memory_matrix.sh measure PROBE OUT [SIZE] | controls PROBE ORACLE OUT [SIZE]" >&2; exit 2 ;;
esac
mkdir -p "$OUT"
sha256sum "$PROBE" > "$OUT/probe.$mode.sha256"
STATUS=0
FIX="$OUT/fixture.png"
[ -f "$FIX" ] || "$PROBE" make "$FIX" "$SIZE" "$SIZE" || exit 1
sha256sum "$FIX" > "$OUT/fixture.$mode.sha256"
cells() {
  for effort in 0 2; do
    for schedule in oneshot live memory disk; do
      echo "$effort $schedule"
    done
  done
}
if [ "$mode" = measure ]; then
  : > "$OUT/peaks.tsv"
  while read -r effort schedule; do
    cell="e${effort}-${schedule}"
    rm -f "$OUT/$cell".out.png "$OUT/$cell".spool
    /usr/bin/time -v -o "$OUT/$cell.time" "$PROBE" run "$FIX" "$OUT/$cell.out.png" "$effort" "$schedule" "$OUT/$cell.spool" \
      > "$OUT/$cell.rss.log" 2>&1
    rc=$?
    rm -f "$OUT/$cell".out.png "$OUT/$cell".spool
    heaptrack -o "$OUT/$cell" "$PROBE" run "$FIX" "$OUT/$cell.out.png" "$effort" "$schedule" "$OUT/$cell.spool" \
      > "$OUT/$cell.heap.log" 2>&1
    f=$(ls "$OUT/$cell".*zst "$OUT/$cell".*gz 2>/dev/null | head -1)
    peak=""
    if [ -n "$f" ]; then
      heaptrack_print -f "$f" > "$OUT/$cell.heap.txt" 2>&1
      peak=$(grep -m1 'peak heap memory consumption' "$OUT/$cell.heap.txt")
    fi
    printf '%s\t%s\t%s\t%s\t%s\n' "$cell" "rc=$rc" "$peak" \
      "$(grep 'Maximum resident' "$OUT/$cell.time")" \
      "$(grep 'Elapsed (wall clock)' "$OUT/$cell.time")" >> "$OUT/peaks.tsv"
    rm -f "$OUT/$cell".out.png "$OUT/$cell".spool
  done < <(cells)
  cat "$OUT/peaks.tsv"
else
  : > "$OUT/controls.tsv"
  outputs=()
  while read -r effort schedule; do
    cell="e${effort}-${schedule}"
    rm -f "$OUT/$cell".out.png "$OUT/$cell".spool
    "$PROBE" run "$FIX" "$OUT/$cell.out.png" "$effort" "$schedule" "$OUT/$cell.spool" > "$OUT/$cell.control.log" 2>&1
    rc=$?
    rm -f "$OUT/$cell".spool
    if [ "$rc" -eq 0 ]; then
      outputs+=("$OUT/$cell.out.png")
      printf '%s\tran\t%s\n' "$cell" "$(tail -1 "$OUT/$cell.control.log")" >> "$OUT/controls.tsv"
    else
      printf '%s\trefused-or-failed rc=%s\t%s\n' "$cell" "$rc" "$(grep -m1 -o 'stage: [A-Za-z]*' "$OUT/$cell.control.log")" >> "$OUT/controls.tsv"
    fi
  done < <(cells)
  "$ORACLE" "$SIZE" "$SIZE" "$FIX" "${outputs[@]}" > "$OUT/oracle.tsv" 2>&1 || STATUS=1
  cat "$OUT/controls.tsv" "$OUT/oracle.tsv"
  rm -f "$OUT"/*.out.png
fi
exit $STATUS
