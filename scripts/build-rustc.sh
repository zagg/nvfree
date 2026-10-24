#!/bin/sh
# nvfree, without cargo: gcc compiles the driver-ABI library, rustc links it.
# Kept deliberately dumb so the tool builds on a bare box (CI, containers).
set -eu

BIN=${BIN:-nvfree}
LIB=${NVSMI_RE:-$HOME/gh-nvsmi}
CC=${CC:-cc}
RUSTC=${RUSTC:-rustc}
AR=${AR:-ar}

if [ ! -f "$LIB/include/fields.h" ]; then
    echo "nvfree: driver-ABI library not found at $LIB" >&2
    echo "         clone it (make vendor) or set NVSMI_RE=/path/to/nvidia-smi-clone-RE" >&2
    exit 1
fi

mkdir -p build
for f in src/nvrm.c src/fields.c; do
    echo "  CC  $LIB/$f"
    $CC -O2 -std=gnu11 -Wall -I"$LIB/include" -c "$LIB/$f" -o "build/$(basename "$f" .c).o"
done
echo "  CC  csrc/nvfree_shim.c"
$CC -O2 -std=gnu11 -Wall -I"$LIB/include" -c csrc/nvfree_shim.c -o build/nvfree_shim.o

echo "  AR  build/libnvf.a"
rm -f build/libnvf.a
$AR rcs build/libnvf.a build/nvrm.o build/fields.o build/nvfree_shim.o

echo "  RUSTC  build/$BIN"
VER=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
CARGO_PKG_VERSION=${VER:-0.1.0} NVF_LIB="$LIB" $RUSTC -O --edition 2021 -C panic=abort -C strip=symbols \
    -L build -l static=nvf -l dylib=dl \
    src/main.rs -o "build/$BIN"

ldd "build/$BIN"
