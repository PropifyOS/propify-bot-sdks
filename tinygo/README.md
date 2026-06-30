# TinyGo guest SDK

The TinyGo toolkit for writing a PropifyOS sandbox bot. You implement the `Bot`
interface (one method, `OnTick`) and write the four ABI export functions by hand (Go
has no macros). The SDK handles the wire codec, the read and emit protocol, and a
static bump arena that keeps the guest off the Go heap.

It targets ABI v3: the `abi_version` export returns `3` and `Bot.OnTick` receives the
v2 `*MarketWindow` and the v3 read-only `*AccountContext` (account status plus the
resolved rule set) alongside the latest snapshot. A snapshot-only bot ignores both.
TinyGo has no native link-section attribute, so the bot's `propify_manifest` section is
injected after the build with the `manifest-encoder` tool (see "Bot manifest" below).

Module path: `github.com/PropifyOS/propify-bot-sdks/tinygo`. Import the SDK as
`github.com/PropifyOS/propify-bot-sdks/tinygo/propify`.

## Prerequisites

- Go 1.24 or later, to run the host-side tests.
- TinyGo 0.41.1 with binaryen, to build the guest. Install locally from the
  [TinyGo guide](https://tinygo.org/getting-started/install/) or use the official
  `tinygo/tinygo:0.41.1` container.
- For a reproducible artifact: `wasm-tools` 1.252.0 on the PATH.

## Layout

- `propify/` is the SDK: the `Bot` interface and tick driver (`bot.go`), the wire codec
  (`wire.go`, `types.go`, `decimal.go`), the static arena allocator (`memory.go`), and
  the host imports. The imports are split by build tag: `imports.go`
  (`//go:build tinygo`) declares the real `//go:wasmimport` bindings, and
  `imports_host.go` (`//go:build !tinygo`) provides trivial stubs so the package still
  compiles and the decoder tests run under the standard Go toolchain.
- `sample/main.go` is the example bot (`package main`).

## Test

```bash
go test ./...
```

This runs under the standard Go toolchain and exercises the host-independent decoders.

## Build a guest module

With a local TinyGo:

```bash
./build-repro.sh
```

Through the container:

```bash
docker run --rm -v "$PWD":/src -w /src tinygo/tinygo:0.41.1 \
  tinygo build -target=wasm-unknown -no-debug -opt=z -panic=trap -scheduler=none \
  -o /src/build/sample.wasm /src/sample
```

The flags: `-target=wasm-unknown` (the `wasm32` target without WASI), `-no-debug`
(strips DWARF, the closest path-sanitisation lever TinyGo offers), `-opt=z`,
`-panic=trap`, and `-scheduler=none`. `build-repro.sh` then normalizes the output with
`wasm-tools strip -d producers`.

## Bot manifest

TinyGo has no native link-section attribute, so the `propify_manifest` custom section is
injected after the build with the `manifest-encoder` tool (in `tools/manifest-encoder`).
The injected bytes are the canonical `BotManifest::encode` output, so the manifest is
byte-identical on rebuild. After building the guest:

```bash
# Encode your descriptor to canonical bytes, then inject the section.
cargo run -p manifest-encoder -- encode bot.manifest.json manifest.bin
cargo run -p manifest-encoder -- inject build/sample.wasm manifest.bin build/sample.with-manifest.wasm
# Confirm exactly one section that decodes cleanly.
cargo run -p manifest-encoder -- verify build/sample.with-manifest.wasm
```

For a reproducible artifact, inject the manifest after `wasm-tools strip -d producers` so
the section bytes are deterministic. See `tools/manifest-encoder/README.md`.

## Two rules to remember

- Use `//export name`, not `//go:wasmexport name`. The newer directive needs the
  reactor lifecycle (`_initialize`), which the host does not drive, so every export
  would trap.
- Do not allocate on the Go heap from an export. Return the address of a package-level
  variable, pass package-level instances into `RunTick`, and avoid `make`, `new`, and
  growing `append`. The host never calls `_initialize`, so the Go allocator is never
  set up; use the SDK's arena-backed helpers.

Pinned toolchain: TinyGo 0.41.1, Go 1.24, wasm-tools 1.252.0.
