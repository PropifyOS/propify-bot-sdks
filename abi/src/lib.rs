//! `propify-sandbox-abi` — the single source of truth for the sandbox ABI codec.
//!
//! The audited boundary between the PropifyOS sandbox host and an untrusted `wasm32`
//! guest is a small, length-prefixed binary format. This crate holds **everything
//! that boundary needs and nothing else**: the six closed domain enums, the five
//! boundary DTOs, the typed [`CodecError`], and the encode/decode primitives. Both
//! sides depend on it, so there is exactly one codec and no chance of host/guest
//! drift.
//!
//! It is deliberately dependency-light and builds on `wasm32-unknown-unknown`: no
//! `ulid`, no `propify-core`, no OS-bound crates. Money is the exact
//! `rust_decimal::Decimal`, never `f64`; it is taken with default features off so the
//! guest does not pull serde through it. `serde` is optional (the `serde` feature),
//! so a guest SDK does not pull it; the feature also re-enables `rust_decimal/serde`,
//! and `propify-core` turns it on to serialize `Decimal` and keep the enums' Propr
//! wire spellings.
//!
//! The one thing intentionally *not* here is bridging an [`OrderIntentBody`] into a
//! full canonical `OrderIntent`: that needs a clock-minted `Ulid` and the
//! `propify-core` types, both denied to the clockless guest, so it stays host-side
//! in `propify-sandbox`.

pub mod enums;
pub mod wire;

pub use enums::{Exchange, OrderSide, OrderType, PositionSide, ProductType, TimeInForce};
pub use wire::{
    AccountView, Candle, CodecError, MAX_CANDLE_COUNT, MAX_MESSAGE_BYTES, MarketSnapshot,
    MarketWindow, OrderIntentBody, StrategyParams,
};

/// The current ABI major version: `2`.
///
/// A guest reports the version it targets from its `abi_version()` export. The host
/// supports v1 and v2 guests side by side: a v1 guest never sees the multi-candle
/// [`MarketWindow`] and runs exactly as before, while a v2 guest may read it. This is
/// dual-support, not a forced migration; the host refuses any version it does not
/// know before a tick runs. The constant lives here, in the shared ABI crate, because
/// it is a property of the boundary contract that both the host and every guest SDK
/// must agree on. v2 is additive over v1: it adds the bounded candle window and the
/// `host_read_market_window` capability, and nothing else.
pub const ABI_VERSION: u32 = 2;
