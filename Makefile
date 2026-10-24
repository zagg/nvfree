# nvfree — a VRAM bar graph that reads the NVIDIA driver directly.
# Build needs: gcc + rust/cargo + the driver-ABI library (nvidia-smi-clone-RE).
# No crates, no NVML, no nvidia-smi.

BIN      ?= nvfree
PREFIX   ?= /usr/local
# where the reverse-engineered driver library lives (see `make vendor`)
NVSMI_RE ?= $(HOME)/gh-nvsmi
export NVSMI_RE

.PHONY: all release debug build rustc run watch once check clean vendor install uninstall help

all: release

release: ## cargo release build (default)
	@cargo build --release
	@printf '\n==> target/release/%s\n' $(BIN)

build: ## debug build
	@cargo build

rustc: ## cargo-free build: plain gcc + rustc, no build.rs
	@sh scripts/build-rustc.sh

run: release ## one snapshot, then exit
	@./target/release/$(BIN)

watch: release ## live bar graph, 1 s refresh, with per-process breakdown
	@./target/release/$(BIN) -l 1 -p

once: release ## fixed-width snapshot, handy for docs and logs
	@./target/release/$(BIN) --width 92

check: release ## cross-validate every number against nvidia-smi (if installed)
	@sh tests/compare.sh

vendor: ## clone the driver-ABI library into ./vendor
	@mkdir -p vendor
	@git clone --depth 1 https://github.com/zagg/nvidia-smi-clone-RE.git vendor/nvidia-smi-clone-RE

clean:
	@cargo clean
	@rm -rf build

install: release
	@install -Dm755 target/release/$(BIN) $(DESTDIR)$(PREFIX)/bin/$(BIN)
	@printf 'installed %s/bin/%s\n' $(PREFIX) $(BIN)

uninstall:
	@rm -f $(DESTDIR)$(PREFIX)/bin/$(BIN)

help:
	@grep -hE '^[a-zA-Z_-]+:.*?## ' Makefile | awk -F'## ' '{printf "  %-9s %s\n", $$1, $$2}'
