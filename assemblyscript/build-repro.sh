#!/usr/bin/env bash
# Single source of truth for the REPRODUCIBLE AssemblyScript guest build (Phase G slice
# G-repro).
#
# It compiles `assembly/sample.ts` to a byte-identical wasm module and normalises it
# (strips the `producers` custom section), so the same source always yields the same
# `.wasm` bytes and therefore the same `ArtifactId` (sha256).
#
# The build-twice-from-two-paths proof (sdks/repro-check.sh), the package.json
# `build:sample` script, and the CI build step all call this script, so the canonical
# recipe lives in exactly one place.
#
# Pinned toolchain: asc 0.28.19 (devDependency, bundles binaryen 130), wasm-tools 1.252.0.
#
# Reproducibility levers, all pinned here:
#   --converge        iterate the optimiser to a fixed point (stable output)
#   --optimizeLevel 3 / --shrinkLevel 1   pinned optimisation knobs
#   --noAssert        no assertion code paths
#   NO --debug, NO --sourceMap            a source map embeds an absolute-path
#                                         `sourceMappingURL`, which would leak the
#                                         build path into the artifact
#
# Usage: build-repro.sh [OUTPUT_WASM]
#   OUTPUT_WASM  where to write the normalised module (default: build/sample.wasm)
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "${script_dir}"

out_wasm="${1:-${script_dir}/build/sample.wasm}"
raw_wasm="${script_dir}/build/sample.raw.wasm"

command -v wasm-tools >/dev/null 2>&1 || {
  echo "build-repro (assemblyscript): wasm-tools not found on PATH; install with" >&2
  echo "  cargo install wasm-tools --version 1.252.0 --locked" >&2
  exit 1
}

# Reproducible install: --frozen-lockfile aborts if pnpm-lock.yaml would change, so asc
# is always the pinned 0.28.19. Served from the pnpm store, so this works offline once
# the store is populated.
pnpm install --frozen-lockfile

mkdir -p "${script_dir}/build"

# Compile with the pinned reproducible flags. `runtime stub` and the `abort` override keep
# the module on the frozen v1 ABI (no `env.abort` host import).
pnpm exec asc assembly/sample.ts \
  --outFile "${raw_wasm}" \
  --runtime stub \
  --noAssert \
  --use abort=assembly/noop/propifyAbort \
  --optimizeLevel 3 \
  --shrinkLevel 1 \
  --converge

# Normalise: strip ONLY the `producers` custom section (tool/version strings). Pure
# deterministic filter; the module stays valid and loadable in wasmtime.
wasm-tools strip -d producers "${raw_wasm}" -o "${out_wasm}"

# OPTIONAL (ABI v3 bot manifest): AssemblyScript has no native link-section attribute, so a
# real bot embeds its `propify_manifest` custom section AFTER this strip step with the
# shared `manifest-encoder` tool (wasm-tools 1.252 has no add-custom-section subcommand).
# The injected bytes are the canonical `BotManifest::encode` output, so the section is
# byte-identical on rebuild and the artifact stays reproducible. Inject LAST (after the
# `producers` strip) so the section bytes are deterministic. This sample ships no manifest,
# so the step is documented here rather than executed; a bot build (for example the Trend
# bot) runs:
#
#   cargo run -p manifest-encoder -- encode bot.manifest.json manifest.bin
#   cargo run -p manifest-encoder -- inject "${out_wasm}" manifest.bin sample.with-manifest.wasm
#   cargo run -p manifest-encoder -- verify sample.with-manifest.wasm
#
# See README.md ("Bot manifest") and tools/manifest-encoder/README.md.

echo "build-repro (assemblyscript): wrote ${out_wasm}"
