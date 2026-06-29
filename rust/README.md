# Rust guest SDK (`propify-bot-sdk`)

The Rust toolkit for writing a PropifyOS sandbox bot and compiling it to
`wasm32-unknown-unknown`. You implement the `Bot` trait and call `register_bot!`; the
SDK generates the ABI exports and handles the read and emit protocol. All `unsafe` is
confined to a wasm32-only FFI module, so your bot code stays safe and unit-testable on
the host target.

It targets ABI v2: `abi_version()` returns `2` and `Bot::on_tick` receives the v2
`MarketWindow` alongside the latest snapshot. A snapshot-only bot ignores the window.

## Prerequisites

- Rust 1.95.0 or later.
- The wasm target: `rustup target add wasm32-unknown-unknown`.
- For a reproducible artifact: `wasm-tools` (`cargo install wasm-tools --version 1.252.0 --locked`).

## Layout

- `src/` is the SDK: the `Bot` trait and tick driver (`bot.rs`), the wasm32 FFI shim
  (`ffi.rs`), and the public surface and `register_bot!` macro (`lib.rs`).
- `examples/minimal_bot.rs` is a complete, minimal starting-point bot, declared
  `crate-type = ["cdylib"]` so it builds to a real guest module.
- It depends on the sibling `abi/` crate by path.

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
