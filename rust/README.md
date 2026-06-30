# Rust guest SDK (`propify-bot-sdk`)

The Rust toolkit for writing a PropifyOS sandbox bot and compiling it to
`wasm32-unknown-unknown`. You implement the `Bot` trait and call `register_bot!`; the
SDK generates the ABI exports and handles the read and emit protocol. All `unsafe` is
confined to a wasm32-only FFI module, so your bot code stays safe and unit-testable on
the host target.

It targets ABI v3: `abi_version()` returns `3` and `Bot::on_tick` receives the v2
`MarketWindow` and the v3 read-only `AccountContext` (account status plus the resolved
rule set) alongside the latest snapshot. A snapshot-only bot ignores both. The bot also
embeds a `propify_manifest` custom section, declared with `declare_manifest!` and emitted
natively by the Rust toolchain (no post-build step).

## Prerequisites

- Rust 1.95.0 or later.
- The wasm target: `rustup target add wasm32-unknown-unknown`.
- For a reproducible artifact: `wasm-tools` (`cargo install wasm-tools --version 1.252.0 --locked`).

## Layout

- `src/` is the SDK: the `Bot` trait and tick driver (`bot.rs`), the wasm32 FFI shim
  (`ffi.rs`), and the public surface, `register_bot!`, and `declare_manifest!` macros
  (`lib.rs`).
- `examples/minimal_bot.rs` is a complete, minimal starting-point bot, declared
  `crate-type = ["cdylib"]` so it builds to a real guest module.
- `build.rs` encodes the example's `BotManifest` into the canonical `propify_manifest`
  bytes; it is the exact pattern a downstream bot crate copies into its own build script.
- It depends on the sibling `abi/` crate by path (as a normal dependency and a
  build-dependency).

## Declaring the bot manifest

ABI v3 carries the bot's identity and metadata (`BotManifest`) inside the artifact as a
`propify_manifest` custom section, hashed together with the code. Unlike the
AssemblyScript and TinyGo SDKs (which inject the section post-build with the
`manifest-encoder` tool), Rust emits it natively from a `#[link_section]` static. Two
steps, both shown in this crate:

1. A `build.rs` that builds a `BotManifest`, calls `.encode()`, and writes the bytes to
   `$OUT_DIR/propify_manifest.bin`. Add `propify-sandbox-abi` under `[build-dependencies]`.
   See this crate's `build.rs` for the template.
2. One `declare_manifest!()` line in the bot, alongside `register_bot!`. The macro embeds
   the encoded bytes in the `propify_manifest` section; it computes the array length from
   the file, so you never hand-count it. `#[link_section]` on a static is safe, so a bot
   crate keeps `unsafe_code = "forbid"`.

The section is gated to `target_arch = "wasm32"`, so it appears only in the guest
artifact. Confirm it after a wasm build:

```bash
wasm-tools print target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm \
  | grep propify_manifest
```

## Test

```bash
cargo test
```

The tests drive the read and emit flow against a mock host, off-target.

## Build a guest module

```bash
cargo build --example minimal_bot --target wasm32-unknown-unknown --release
```

The module is written to
`target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm`. It exports `memory`,
`abi_version`, `alloc`, `dealloc`, and `on_tick`, and imports only the `propify`
capabilities.

To start your own bot, copy `examples/minimal_bot.rs` into a new crate with
`crate-type = ["cdylib"]` and `propify-bot-sdk` as a dependency, then edit the
`on_tick` body.

## Reproducible build

A published artifact must be byte-identical so its `ArtifactId` (sha256) can be
verified. Build with the `release` profile, remap absolute paths so the output does not
depend on the checkout location, and strip the `producers` section:

```bash
CARGO_INCREMENTAL=0 \
CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$PWD=/workspace"$'\x1f'"--remap-path-scope=object" \
  cargo build --example minimal_bot --target wasm32-unknown-unknown --release
wasm-tools strip -d producers \
  target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm \
  -o minimal_bot.wasm
```

Pinned toolchain: rustc 1.95.0, wasm-tools 1.252.0.
