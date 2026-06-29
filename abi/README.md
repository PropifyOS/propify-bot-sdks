# `propify-sandbox-abi`

The wire contract for the PropifyOS bot sandbox: the byte format and the boundary types
that the host and the guest SDKs both build on. This is the single source of truth for
the codec, so the host and every guest agree byte for byte.

It is a small, safe Rust crate (`unsafe_code = "forbid"`) that builds on
`wasm32-unknown-unknown`. The Rust guest SDK depends on it by path; the AssemblyScript
and TinyGo SDKs reimplement the same format in their own languages and are kept in step
with the round-trip tests here.

## What it defines

- `ABI_VERSION` (currently `2`).
- The boundary DTOs: `MarketSnapshot`, `MarketWindow` and `Candle` (ABI v2),
  `StrategyParams`, `AccountView`, and `OrderIntentBody`.
- The six wire enums with their frozen discriminants: `Exchange`, `ProductType`,
  `OrderSide`, `PositionSide`, `OrderType`, `TimeInForce`.
- The encode and decode functions and `CodecError`.
- The size bounds: `MAX_MESSAGE_BYTES` (64 KiB) and `MAX_CANDLE_COUNT` (256).

## Build and test

```bash
cargo build
cargo test
```

## Features

`serde` is off by default so a wasm guest never pulls serde. A host that wants to
serialize the types or the `Decimal` money type can enable the `serde` feature.

## Money

Money is the exact `rust_decimal::Decimal`, carried on the wire as a 16-byte `i128`
mantissa plus a 4-byte `i32` scale. No `f64` ever touches the boundary.
