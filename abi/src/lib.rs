//! `propify-sandbox-abi` — the single source of truth for the sandbox ABI codec.
//!
//! The audited boundary between the PropifyOS sandbox host and an untrusted `wasm32`
//! guest is a small, length-prefixed binary format. This crate holds **everything
//! that boundary needs and nothing else**: the eight closed domain enums, the seven
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

pub use enums::{
    AccountStatus, DrawdownKind, Exchange, OrderSide, OrderType, PositionSide, ProductType,
    TimeInForce,
};
pub use wire::{
    AccountContext, AccountView, BotManifest, Candle, CodecError, DrawdownRule, MAX_CANDLE_COUNT,
    MAX_MANIFEST_BYTES, MAX_MESSAGE_BYTES, MarketSnapshot, MarketWindow, OrderIntentBody,
    StrategyParams,
};

/// The current ABI major version: `3`.
///
/// A guest reports the version it targets from its `abi_version()` export. v3 is the
/// single supported version: the host accepts only guests reporting `3` and the v1/v2
/// dual-support path is dropped. Nothing is released, so there is no
/// backward-compatibility requirement to carry forward. The constant lives here, in
/// the shared ABI crate, because it is a property of the boundary contract that both
/// the host and every guest SDK must agree on.
///
/// v3 adds exactly two things over v2: an embedded [`BotManifest`], carried as a
/// `propify_manifest` wasm custom section the host extracts and validates during the
/// static scan, and a read-only [`AccountContext`] (account lifecycle status plus the
/// resolved rule set) the guest reads each tick through a new host capability.
/// [`MAX_CANDLE_COUNT`] stays at 256.
pub const ABI_VERSION: u32 = 3;
