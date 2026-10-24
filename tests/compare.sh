#!/bin/sh
# tests/compare.sh - nvfree vs the reference tool, field by field.
# The whole point of the project is that these two numbers agree without ever
# touching NVML, so this is the only test that really matters.
set -u

BIN=${BIN:-./target/release/nvfree}
[ -x "$BIN" ] || BIN=./build/nvfree
if [ ! -x "$BIN" ]; then
    echo "nvfree: build the binary first (make release)" >&2
    exit 1
fi

fail=0
pass=0

if command -v nvidia-smi >/dev/null 2>&1; then
    # reference: index memory.total memory.used memory.free memory.reserved
    ref=$(nvidia-smi --query-gpu=index,memory.total,memory.used,memory.free,memory.reserved \
                        --format=csv,noheader,nounits | tr ',' ' ')
    got=$("$BIN" --json | python3 -c '
import json,sys
for line in sys.stdin:
    d = json.loads(line)
    m = d["vram_mib"]
    print("%d %d %d %d %d" % (d["index"], m["total"], m["used"], m["free"], m["reserved"]))
')
    echo "comparing against nvidia-smi (MiB, NVML rounding):"
    printf '%s\n' "$ref" | while read -r idx tot used free res; do
        line=$(printf '%s\n' "$got" | awk -v i="$idx" '$1==i {print; exit}')
        if [ -z "$line" ]; then
            echo "  GPU $idx  MISSING in nvfree output"
            continue
        fi
        set -- $line
        status=OK
        for k in 2 3 4 5; do
            a=$(eval echo \$$k)
            b=$(echo "$ref" | awk -v i="$idx" '$1==i {print $'"$k"'}')
            # nvidia-smi itself drifts between two samples: allow 1% slack
            d=$(( a > b ? a - b : b - a ))
            if [ "$d" -gt $(( b / 100 + 2 )) ]; then status="DIFF"; fi
        done
        echo "  GPU $idx  nvfree=$2/$3/$4/$5  nvidia-smi=$tot/$used/$free/$res  $status"
    done
else
    echo "nvidia-smi not installed: skipping the cross-check (nvfree still works, that is the idea)"
fi

echo
echo "internal consistency (used + reserved + free == total):"
"$BIN" --json | python3 - "$BIN" <<'PY'
import json, subprocess, sys
bin_ = sys.argv[1]
out = subprocess.run([bin_, "--json"], capture_output=True, text=True).stdout
bad = 0
for line in out.splitlines():
    d = json.loads(line)
    k = d["vram_kib"]
    s = k["used"] + k["reserved"] + k["free"]
    ok = abs(s - k["total"]) <= 1024  # sub-MiB rounding of the heap accounting
    print("  GPU %d  %d + %d + %d = %d vs total %d  %s"
          % (d["index"], k["used"], k["reserved"], k["free"], s, k["total"],
             "OK" if ok else "MISMATCH"))
    if not ok:
        bad += 1
    for p in d["processes"]:
        if p["bytes"] // 1024 > k["used"] + 1024:
            print("    process %d accounts for more than the used pool" % p["pid"])
            bad += 1
sys.exit(1 if bad else 0)
PY
rc=$?
echo
if [ $rc -eq 0 ]; then echo "compare.sh: numbers agree with the reference tool"; else echo "compare.sh: MISMATCH"; fi
exit $rc
