#!/usr/bin/env bash
# Single source of truth for the REPRODUCIBLE TinyGo guest build (Phase G slice G-repro).
#
# It compiles the TinyGo sample to a byte-identical wasm module and normalises it (strips
# the `producers` custom section), so the same source always yields the same `.wasm` bytes
# and therefore the same `ArtifactId` (sha256), for a fixed toolchain.
#
# The build-twice-from-two-paths proof (sdks/repro-check.sh) and the CI build step both
# call this script, so the canonical recipe lives in exactly one place.
#
# Pinned toolchain: tinygo 0.41.1 (bundles its own wasm-opt/LLVM), wasm-tools 1.252.0.
# IMPORTANT caveat: TinyGo has no `-trimpath` and gives no official reproducibility
# guarantee. `-no-debug` removes the DWARF that embeds GOROOT/GOPATH/module-cache absolute
# paths (the closest available path sanitiser), and `-panic=trap` removes panic message
# strings. Byte-identity therefore also depends on a fixed Go toolchain version and the
# fixed TinyGo release (go.mod pins go 1.24; CI pins tinygo 0.41.1).
#
# Usage: build-repro.sh [OUTPUT_WASM]
#   OUTPUT_WASM  where to write the normalised module (default: build/sample.wasm)
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "${script_dir}"

out_wasm="${1:-${script_dir}/build/sample.wasm}"
raw_wasm="${script_dir}/build/sample.raw.wasm"

command -v tinygo >/dev/null 2>&1 || {
  echo "build-repro (tinygo): tinygo not found on PATH (need 0.41.1)" >&2
  exit 1
}
command -v wasm-tools >/dev/null 2>&1 || {
  echo "build-repro (tinygo): wasm-tools not found on PATH; install with" >&2
  echo "  cargo install wasm-tools --version 1.252.0 --locked" >&2
  exit 1
}

mkdir -p "${script_dir}/build"

# Reproducible flags for the frozen v1 ABI on the wasm-unknown target:
#   -no-debug      strip DWARF (the only path-sanitisation lever TinyGo offers)
#   -opt=z         size-optimised, pinned
#   -panic=trap    panics become `unreachable`, no message strings embedded
#   -scheduler=none  no goroutine scheduler (the sample is single-shot, fully sync);
#                    also removes scheduler init the wasm-unknown host never drives
tinygo build \
  -target=wasm-unknown \
  -no-debug \
  -opt=z \
  -panic=trap \
  -scheduler=none \
  -o "${raw_wasm}" \
  "${script_dir}/sample"

# Normalise: strip ONLY the `producers` custom section. Pure deterministic filter; the
# module stays valid and loadable in wasmtime.
wasm-tools strip -d producers "${raw_wasm}" -o "${out_wasm}"

echo "build-repro (tinygo): wrote ${out_wasm}"
