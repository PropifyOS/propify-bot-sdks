# AssemblyScript guest SDK

The AssemblyScript toolkit for writing a PropifyOS sandbox bot. You subclass `Bot`,
implement `onTick`, and write the four ABI export functions by hand (AssemblyScript has
no macros). The SDK handles the wire codec and the read and emit protocol.

It targets ABI v3: the `abi_version` export returns `3` and `Bot.onTick` receives the v2
`MarketWindow` and the v3 read-only `AccountContext` (account status plus the resolved
rule set) alongside the latest snapshot. A snapshot-only bot ignores both. AssemblyScript
has no native link-section attribute, so the bot's `propify_manifest` section is injected
after the build with the `manifest-encoder` tool (see "Bot manifest" below).

## Prerequisites

- Node.js 24.
- pnpm 11.9.0.
- AssemblyScript 0.28.19, installed from the lockfile (no global install needed).
- For a reproducible artifact: `wasm-tools` 1.252.0 on the PATH.

## Layout

- `assembly/` is the SDK plus the example bot. The SDK files are `index.ts`,
  `types.ts`, `decimal.ts`, `wire.ts`, `imports.ts`, and `noop.ts`. The example bot is
  `sample.ts`.
- `test/` holds the Node-based decoder and sample tests.
- `build-repro.sh` is the single source of truth for the compiler flags.

## Install, build, test

```bash
pnpm install --frozen-lockfile
pnpm build
pnpm test
```

`pnpm build` runs `build-repro.sh` and writes `build/sample.wasm`. `pnpm test` runs the
SDK tests with Node's test runner.

## Two flags that are required, not optional

The build passes these for correctness, not just size:

- `--runtime stub`: the bump-allocator runtime. The default GC runtime imports
  `env.abort`, which is not on the host allow-list and would be refused at load.
- `--use abort=assembly/noop/propifyAbort`: replaces the default `abort` with a no-op,
  so a runtime assertion cannot call into the environment.

The `memory` export is emitted automatically because the build does not pass
`--importMemory`.

## Bot manifest

AssemblyScript has no native link-section attribute, so the `propify_manifest` custom
section is injected after the build with the `manifest-encoder` tool (in
`tools/manifest-encoder`). The injected bytes are the canonical `BotManifest::encode`
output, so the manifest is byte-identical on rebuild. After building the guest:

```bash
# Encode your descriptor to canonical bytes, then inject the section.
cargo run -p manifest-encoder -- encode bot.manifest.json manifest.bin
cargo run -p manifest-encoder -- inject build/sample.wasm manifest.bin build/sample.with-manifest.wasm
# Confirm exactly one section that decodes cleanly.
cargo run -p manifest-encoder -- verify build/sample.with-manifest.wasm
```

For a reproducible artifact, inject the manifest after `wasm-tools strip -d producers` (the
last step of `build-repro.sh`) so the section bytes are deterministic. See
`tools/manifest-encoder/README.md`. The sample ships no manifest, so the default build does
not inject one; a real bot build runs the steps above.

## Start your own bot

Copy `assembly/sample.ts`, rename the class, and point the `asc` entry (in
`asconfig.json` and `build-repro.sh`) at your file. Keep the four hand-written ABI
exports and the `import "./noop";` line.

Pinned toolchain: asc 0.28.19 (bundles binaryen), wasm-tools 1.252.0.
