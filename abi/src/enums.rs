//! The six closed enumerations carried on the sandbox boundary.
//!
//! These mirror the Propr API "Enums" section (see `docs/specs/propr-api.md`).
//! Modeling them as Rust enums rather than free strings makes invalid states
//! unrepresentable: a typo like `"byu"` cannot exist in the type system, and the
//! risk and execution layers can `match` exhaustively without a fallback arm.
//!
//! They live in `propify-sandbox-abi` (not `propify-core`) because they are part of
//! the wire boundary the host and guest SDKs share. `propify-core` re-exports them,
//! so the *same type identity* is preserved for every other workspace crate.
//!
//! `serde` is behind the optional `serde` feature so a wasm guest never pulls it.
//! When enabled, the derives use the exact wire spellings Propr expects (lowercase
//! for most, uppercase for `TimeInForce`, snake_case for `OrderType`), so these
//! types double as the serialization contract. The discriminant meanings are stable
//! and must not be renumbered; the codec in [`crate::wire`] encodes them by value.

/// Venue the order is routed to.
///
/// Propr v1 only funds Hyperliquid, but the adapter pattern keeps this open so
/// other venues can be added without reshaping the domain (spec §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Exchange {
    Hyperliquid,
}

/// Instrument family. Propr distinguishes spot from perpetual futures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ProductType {
    Spot,
    Perp,
}

/// Direction of an order at the book level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Side of the resulting position.
///
/// Kept distinct from [`OrderSide`] on purpose: selling can either open a short
/// or reduce a long, so the position side is not derivable from the order side
/// alone (see the `reduceOnly` safety note in `docs/specs/propr-api.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum PositionSide {
    Long,
    Short,
}

/// Order type. The required price/trigger fields differ per variant; that
/// validation lives in the adapter/risk layers, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum OrderType {
    Market,
    Limit,
    StopMarket,
    StopLimit,
    TakeProfitMarket,
    TakeProfitLimit,
}

/// Time-in-force policy. Propr sends these uppercase on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "UPPERCASE"))]
pub enum TimeInForce {
    /// Good til cancelled (Propr default).
    Gtc,
    /// Immediate or cancel (used for market orders).
    Ioc,
    /// Fill or kill.
    Fok,
    /// Post-only / maker (`GTX` on the wire).
    Gtx,
}

// The serde wire-spelling tests for these six enums live in `propify-core`'s
// `enums.rs` (it enables the `serde` feature and exercises the re-exported types),
// so they are not duplicated here. Keeping them in one place avoids a second
// `serde_json` dependency and a divergent copy of the same assertions.
