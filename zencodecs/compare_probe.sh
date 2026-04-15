#!/bin/bash
# Compare zcimg info --json vs identify for image corpuses.
# Outputs mismatches in dimensions, format, or alpha detection.

ZCIMG="/home/lilith/work/zencodecs/zcimg/target/release/zcimg"
OUTDIR="/mnt/v/output/zencodecs/probe-compare"
mkdir -p "$OUTDIR"

CORPUS_DIRS=(
    "/mnt/v/GitHub/codec-corpus"
    "/mnt/v/work/corpus"
)

TOTAL=0
ZCIMG_OK=0
ZCIMG_FAIL=0
IDENTIFY_OK=0
IDENTIFY_FAIL=0
DIM_MATCH=0
DIM_MISMATCH=0
BOTH_FAIL=0

echo "file,zcimg_w,zcimg_h,zcimg_format,zcimg_alpha,zcimg_icc,zcimg_exif,identify_w,identify_h,identify_format,identify_alpha,match" > "$OUTDIR/results.csv"

for dir in "${CORPUS_DIRS[@]}"; do
    while IFS= read -r -d '' file; do
        TOTAL=$((TOTAL + 1))

        # Get extension for filtering
        ext="${file##*.}"
        ext_lower=$(echo "$ext" | tr '[:upper:]' '[:lower:]')

        # Run zcimg info --json
        zcimg_json=$("$ZCIMG" info "$file" --json 2>/dev/null)
        zcimg_rc=$?

        if [ $zcimg_rc -eq 0 ] && [ -n "$zcimg_json" ]; then
            ZCIMG_OK=$((ZCIMG_OK + 1))
            zw=$(echo "$zcimg_json" | jq -r '.width // empty')
            zh=$(echo "$zcimg_json" | jq -r '.height // empty')
            zfmt=$(echo "$zcimg_json" | jq -r '.format // empty')
            zalpha=$(echo "$zcimg_json" | jq -r '.has_alpha // empty')
            zicc=$(echo "$zcimg_json" | jq -r '.icc_profile_size // "null"')
            zexif=$(echo "$zcimg_json" | jq -r '.exif_size // "null"')
        else
            ZCIMG_FAIL=$((ZCIMG_FAIL + 1))
            zw=""
            zh=""
            zfmt=""
            zalpha=""
            zicc=""
            zexif=""
        fi

        # Run identify -verbose (just grab key fields)
        id_out=$(identify -format '%w %h %m %A' "$file" 2>/dev/null | head -1)
        id_rc=$?

        if [ $id_rc -eq 0 ] && [ -n "$id_out" ]; then
            IDENTIFY_OK=$((IDENTIFY_OK + 1))
            iw=$(echo "$id_out" | awk '{print $1}')
            ih=$(echo "$id_out" | awk '{print $2}')
            ifmt=$(echo "$id_out" | awk '{print $3}')
            ialpha=$(echo "$id_out" | awk '{print $4}')
        else
            IDENTIFY_FAIL=$((IDENTIFY_FAIL + 1))
            iw=""
            ih=""
            ifmt=""
            ialpha=""
        fi

        # Compare dimensions
        if [ -n "$zw" ] && [ -n "$iw" ]; then
            if [ "$zw" = "$iw" ] && [ "$zh" = "$ih" ]; then
                match="yes"
                DIM_MATCH=$((DIM_MATCH + 1))
            else
                match="MISMATCH"
                DIM_MISMATCH=$((DIM_MISMATCH + 1))
                echo "DIM MISMATCH: $file  zcimg=${zw}x${zh}  identify=${iw}x${ih}"
            fi
        elif [ -z "$zw" ] && [ -z "$iw" ]; then
            match="both_fail"
            BOTH_FAIL=$((BOTH_FAIL + 1))
        elif [ -z "$zw" ]; then
            match="zcimg_fail"
            echo "ZCIMG FAIL: $file (identify: ${iw}x${ih} $ifmt)"
        else
            match="identify_fail"
        fi

        # Log to CSV
        echo "\"$file\",$zw,$zh,$zfmt,$zalpha,$zicc,$zexif,$iw,$ih,$ifmt,$ialpha,$match" >> "$OUTDIR/results.csv"

    done < <(find "$dir" -type f \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.gif" -o -iname "*.avif" -o -iname "*.jxl" \) -print0 2>/dev/null)
done

echo ""
echo "=== SUMMARY ==="
echo "Total files:       $TOTAL"
echo "zcimg succeeded:   $ZCIMG_OK"
echo "zcimg failed:      $ZCIMG_FAIL"
echo "identify succeeded: $IDENTIFY_OK"
echo "identify failed:    $IDENTIFY_FAIL"
echo "Dimension match:   $DIM_MATCH"
echo "Dimension mismatch: $DIM_MISMATCH"
echo "Both failed:       $BOTH_FAIL"
echo ""
echo "Results: $OUTDIR/results.csv"

# Show zcimg-only failures (identify succeeded but zcimg didn't)
echo ""
echo "=== ZCIMG FAILURES (identify succeeded) ==="
grep "zcimg_fail" "$OUTDIR/results.csv" | head -20

# Show dimension mismatches
echo ""
echo "=== DIMENSION MISMATCHES ==="
grep "MISMATCH" "$OUTDIR/results.csv" | head -20

# Show metadata summary
echo ""
echo "=== METADATA COVERAGE ==="
echo "Files with ICC profile: $(grep -v 'null' "$OUTDIR/results.csv" | grep -c 'icc_profile_size' || echo 0) (approximate)"
awk -F',' 'NR>1 && $6 != "null" && $6 != "" {icc++} NR>1 && $7 != "null" && $7 != "" {exif++} END {print "ICC detected: " icc+0; print "EXIF detected: " exif+0}' "$OUTDIR/results.csv"
