//! The stable wire format and codec (spec §7.D).
//!
//! This module is the *audited boundary* between the host and untrusted guest bot
//! code. It is deliberately **language-agnostic**: the only things that cross the
//! boundary are `i32` pointer/length numbers (host-function params) and the byte
//! layouts documented below. No Rust-specific type information is ever serialized,
//! so a guest SDK in Rust, AssemblyScript, or TinyGo can target this format from
//! the spec alone, without reading host internals.
//!
//! # Why a hand-rolled binary codec (not serde)
//!
//! The boundary must be portable to languages that have no serde, and every byte
//! must be accounted for so a hostile guest cannot smuggle ambiguity through the
//! decoder. A small, explicit, length-prefixed schema gives us a decoder that is
//! total (never panics, never reads out of bounds) and returns a *typed*
//! [`CodecError`] for every malformed input. Money is carried as an exact
//! `(i128 mantissa, i32 scale)` pair — **never** `f64`/`f32` — so a guest's live
//! behaviour reproduces its backtest bit-for-bit (spec §7.D determinism).
//!
//! # Primitive encodings (all integers little-endian)
//!
//! | Type        | Bytes | Encoding                                                |
//! |-------------|-------|---------------------------------------------------------|
//! | `u8`        | 1     | the byte                                                |
//! | `u16`/`u32` | 2/4   | little-endian                                           |
//! | `i32`/`i64` | 4/8   | little-endian, two's complement                         |
//! | `i128`      | 16    | little-endian, two's complement                         |
//! | `bool`      | 1     | `0` = false, `1` = true; any other byte is rejected     |
//! | enum        | 1     | discriminant byte; an unknown value is rejected         |
//! | `Option<T>` | 1+?   | tag `0` = None, `1` = Some + `T`; other tag rejected     |
//! | `String`    | 4+n   | `u32` LE byte length `n`, then `n` UTF-8 bytes           |
//! | `Decimal`   | 20    | `i128` mantissa (16 LE) + `i32` scale (4 LE)            |
//!
//! A `Decimal` is decoded with the non-panicking
//! [`Decimal::try_from_i128_with_scale`]; a scale outside `0..=28` or a mantissa
//! beyond the 96-bit magnitude is rejected as [`CodecError::DecimalOutOfRange`],
//! never a panic.
//!
//! # Enum discriminants (stable; do not renumber)
//!
//! - `Exchange`: `Hyperliquid` = 0
//! - `ProductType`: `Spot` = 0, `Perp` = 1
//! - `OrderSide`: `Buy` = 0, `Sell` = 1
//! - `PositionSide`: `Long` = 0, `Short` = 1
//! - `OrderType`: `Market` = 0, `Limit` = 1, `StopMarket` = 2, `StopLimit` = 3,
//!   `TakeProfitMarket` = 4, `TakeProfitLimit` = 5
//! - `TimeInForce`: `Gtc` = 0, `Ioc` = 1, `Fok` = 2, `Gtx` = 3
//! - `AccountStatus` (ABI v3): `Evaluation` = 0, `Funded` = 1
//! - `DrawdownKind` (ABI v3): `Static` = 0, `Trailing` = 1
//!
//! # Message layouts
//!
//! - **`MarketSnapshot`**: `asset: String`, `timestamp_ms: i64`, then five
//!   `Decimal`s in order `open`, `high`, `low`, `close`, `volume`.
//! - **`MarketWindow`** (ABI v2): `asset: String`, `count: u32`, then `count`
//!   candles. Each candle is `timestamp_ms: i64` then the same five OHLCV `Decimal`s
//!   (108 bytes per candle), ordered oldest to newest. The count is capped at
//!   [`MAX_CANDLE_COUNT`]; an over-cap count is rejected as [`CodecError::Oversized`].
//! - **`StrategyParams`**: `count: u32`, then `count` pairs of `(name: String,
//!   value: Decimal)`.
//! - **`AccountView`**: four `Decimal`s in order `equity`, `available_margin`,
//!   `exposure`, `unrealized_pnl`. This account's own figures only — there is no
//!   account id and no other-account data on the wire.
//! - **`OrderIntentBody`**: `exchange`, `asset: String`, `product_type`, `side`,
//!   `position_side`, `order_type`, `time_in_force`, `quantity: Decimal`,
//!   `price: Option<Decimal>`, `trigger_price: Option<Decimal>`,
//!   `reduce_only: bool`. It carries **no** `intent_id` and **no** account: the
//!   clockless guest cannot mint a ULID and must not name an account, so the host
//!   stamps the `intent_id` from its tick context when it lifts the body into a
//!   full `OrderIntent` (done host-side in `propify-sandbox`, which owns the
//!   `OrderIntent` type and the `Ulid` clock the guest is denied).
//! - **`AccountContext`** (ABI v3): `status: AccountStatus`, `daily_loss_limit:
//!   Decimal`, `daily_loss_floor: Decimal`, then the inline `DrawdownRule` (`kind:
//!   DrawdownKind`, `limit: Decimal`, `floor: Decimal`, `high_water_mark: Decimal`),
//!   `default_leverage: Decimal`, a `u32`-count-prefixed `leverage_overrides` list of
//!   `(asset_class: String, cap: Decimal)` pairs, and a `u32`-count-prefixed
//!   `allowed_instruments` list of `String`s. The host resolves and encodes it per
//!   tick; the guest only ever reads the resolved snapshot.
//! - **`BotManifest`** (ABI v3): `name: String`, `description: String`, `version:
//!   String`, `license: String`, `image_sha256: Option<[u8; 32]>` (tag byte then 32
//!   raw bytes when present), `author_name: String`, `author_email: String`,
//!   `author_erc20: String`, `source_repo_url: String`. The SDK emits it at build time
//!   into the `propify_manifest` custom section; the codec does a structural decode
//!   with per-field byte caps only. Semantic validation (semver, SPDX, EIP-55, URL,
//!   email) lives marketplace-side, not here.
//!
//! # Host-function protocol (the five capabilities)
//!
//! All five host functions take `(ptr: i32, len: i32)` and return `i32`.
//!
//! - **Read** (`host_read_market_data`, `host_read_market_window` (ABI v2),
//!   `host_read_strategy_params`, `host_read_account_view`): the guest passes a
//!   destination buffer `(ptr, len)`. The host encodes the message (the single-candle
//!   snapshot, the multi-candle window, the params, or the account view) and returns
//!   its full length `n`. If `len < n` the host writes **nothing** and returns `n`, so
//!   the guest can re-`alloc` and retry. `-1` signals an internal memory error.
//! - **Emit** (`host_emit_intent`): the guest passes `(ptr, len)` of an encoded
//!   `OrderIntentBody`. The host reads those bytes, decodes them with this codec,
//!   bounds-checks the intent, and on success records it. Return values: `0`
//!   accepted, `-1` out-of-bounds/short guest read, `-2` codec decode error, `-3`
//!   intent bounds-check rejection.
//!
//! Every top-level decode enforces an overall size cap ([`CodecError::Oversized`])
//! and requires the buffer to be **fully consumed**, rejecting trailing bytes
//! ([`CodecError::TrailingBytes`]) so no guest can append hidden data.

use crate::enums::{
    AccountStatus, DrawdownKind, Exchange, OrderSide, OrderType, PositionSide, ProductType,
    TimeInForce,
};
use rust_decimal::Decimal;
use thiserror::Error;

/// Overall cap on a single encoded message, in bytes.
///
/// A market snapshot or an intent is well under a kilobyte; this ceiling bounds the
/// host's decode work and the buffer the host allocates for a guest emission, so a
/// hostile `len` cannot trigger a large allocation before the codec even runs.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Cap on a single `String` field's byte length.
///
/// Asset symbols and parameter names are tiny; this rejects an absurd length prefix
/// before any bytes are read.
const MAX_STRING_BYTES: usize = 1024;

/// Cap on the number of `(name, value)` pairs in a [`StrategyParams`].
///
/// Bounds the decode loop deterministically so a huge `count` cannot drive work or
/// allocation; an over-cap count is rejected up front.
const MAX_PARAM_COUNT: usize = 1024;

/// Cap on the number of candles in a [`MarketWindow`] (ABI v2).
///
/// CEO-locked at 256 (a fixed ceiling, never an install parameter). It bounds the
/// v2 window-read decode loop deterministically and keeps the worst-case encoded
/// window comfortably under [`MAX_MESSAGE_BYTES`]: 256 candles × 108 bytes each +
/// the asset string + the count prefix is ~28.7 KiB, well below the 64 KiB cap. An
/// over-cap count is rejected up front, exactly as the strategy-parameter count is.
pub const MAX_CANDLE_COUNT: usize = 256;

/// Cap on the encoded [`BotManifest`] section payload, in bytes (ABI v3).
///
/// 8 KiB, well under the 1 MiB per-custom-section scanner cap. It is exported so the
/// monorepo static scanner can refuse an oversize `propify_manifest` section *before*
/// it ever calls [`BotManifest::decode`]; the codec's own backstop stays the 64 KiB
/// [`MAX_MESSAGE_BYTES`] enforced by every top-level decode. The codec does not
/// enforce this cap itself — keeping the manifest decode a dumb structural read.
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024;

// --- Per-field byte caps for the v3 BotManifest (decode-side, structural only) -----
//
// These bound each manifest string before its bytes are read, mirroring the existing
// `MAX_STRING_BYTES` discipline. They are the structural byte caps only: semantic
// validation (semver, SPDX, EIP-55, URL, email) is a marketplace concern and lives
// host-side, not in this codec. Two fields exceed the global `MAX_STRING_BYTES` of
// 1024 (`description`) so the codec reads manifest strings via `read_string_capped`.

/// `name` cap: a human-readable bot name fits comfortably in 100 bytes.
const MAX_MANIFEST_NAME_BYTES: usize = 100;

/// `description` cap: a short description, the one field that exceeds the global
/// `MAX_STRING_BYTES` (1024), so `read_string_capped` is required for it.
const MAX_MANIFEST_DESCRIPTION_BYTES: usize = 2000;

/// `version` cap: the design gives no explicit cap for the semver string; 64 bytes is
/// safe headroom for any valid `MAJOR.MINOR.PATCH` plus pre-release/build metadata.
const MAX_MANIFEST_VERSION_BYTES: usize = 64;

/// `license` cap: a single SPDX identifier is short; 64 bytes is ample.
const MAX_MANIFEST_LICENSE_BYTES: usize = 64;

/// `author_name` cap: a self-declared author name, same bound as `name`.
const MAX_MANIFEST_AUTHOR_NAME_BYTES: usize = 100;

/// `author_email` cap: 254 bytes, the maximum length of an email address.
const MAX_MANIFEST_AUTHOR_EMAIL_BYTES: usize = 254;

/// `author_erc20` cap: 42 bytes (`0x` plus 40 hex digits). The exact-length and
/// EIP-55 checksum checks are semantic and run marketplace-side; here it is a byte cap.
const MAX_MANIFEST_AUTHOR_ERC20_BYTES: usize = 42;

/// `source_repo_url` cap: 512 bytes for the https repository link.
const MAX_MANIFEST_SOURCE_REPO_URL_BYTES: usize = 512;

// --- Bounds for the v3 AccountContext lists (decode-side) --------------------------

/// Cap on the number of `(asset_class, cap)` leverage overrides in an
/// [`AccountContext`].
///
/// The Propr rulebook defines leverage at the asset-class level — BTC/ETH perps,
/// other-crypto perps, equities perps, commodities perps — so only a handful of
/// classes ever appear. 64 is generous headroom over that ~4-class reality while still
/// bounding the decode loop deterministically; an over-cap count is rejected up front.
const MAX_LEVERAGE_OVERRIDE_COUNT: usize = 64;

/// Cap on the number of `allowed_instruments` in an [`AccountContext`].
///
/// Allowed markets are the Hyperliquid perpetual futures; 1024 bounds the list well
/// above the current venue's instrument count and matches the [`MAX_PARAM_COUNT`]
/// discipline. An over-cap count is rejected up front.
const MAX_ALLOWED_INSTRUMENT_COUNT: usize = 1024;

/// Cap on a leverage-override `asset_class` string, in bytes.
///
/// Asset-class keys (for example `"BTC"`, `"crypto"`, `"equities"`) are short; 64
/// bytes rejects an absurd length prefix while accepting every rulebook class.
const MAX_ASSET_CLASS_BYTES: usize = 64;

/// Why an encoded boundary message could not be decoded.
///
/// The decoder is *total*: it returns one of these typed variants for every
/// malformed input and never panics nor reads out of bounds. `PartialEq`/`Eq` make
/// the failure directly assertable in tests, and `Clone` lets a test collect
/// outcomes across runs. The last variant, [`CodecError::IntentRejected`], carries
/// a post-decode structural bounds-check failure (the value decoded cleanly but is
/// not a sane order), kept here so a single typed channel describes every reason a
/// guest emission can be refused.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The buffer ended before a field could be fully read. The decoder stops here
    /// rather than reading past the end.
    #[error("buffer underrun while decoding a field")]
    ShortBuffer,

    /// The top-level message decoded, but bytes remained after it. Rejected so a
    /// guest cannot append hidden data to a valid message.
    #[error("trailing bytes after a fully decoded message")]
    TrailingBytes,

    /// The message, or a length-prefixed field within it, exceeds its size cap and
    /// was refused before the bytes were processed.
    #[error("encoded payload is {size} bytes, exceeding the {limit}-byte cap")]
    Oversized {
        /// The claimed/observed size, in bytes.
        size: usize,
        /// The configured cap, in bytes.
        limit: usize,
    },

    /// A `bool` or an `Option` tag byte was neither `0` nor `1`.
    #[error("invalid bool/tag byte {value} (expected 0 or 1)")]
    BadBool {
        /// The offending byte.
        value: u8,
    },

    /// An enum discriminant byte did not match any known variant.
    #[error("unknown {kind} discriminant {value}")]
    UnknownDiscriminant {
        /// The enum being decoded (for example `"OrderType"`).
        kind: &'static str,
        /// The offending discriminant byte.
        value: u8,
    },

    /// A `String` field's bytes were not valid UTF-8.
    #[error("invalid UTF-8 in a string field")]
    BadUtf8,

    /// A `Decimal`'s `(mantissa, scale)` is outside the representable range (scale
    /// above 28, or a mantissa beyond the 96-bit magnitude). Validated via
    /// [`Decimal::try_from_i128_with_scale`], so this is a typed error, never a
    /// panic.
    #[error("decimal (mantissa, scale) is out of the representable range")]
    DecimalOutOfRange,

    /// The bytes decoded into a well-formed [`OrderIntentBody`], but the intent
    /// failed the host's boundary bounds-check (for example a non-positive
    /// quantity). This is the `-3` emit status; the codec itself succeeded.
    #[error("intent rejected by the boundary bounds-check: {reason}")]
    IntentRejected {
        /// A fixed, English explanation of which invariant was violated.
        reason: &'static str,
    },
}

/// A bounds-checked, forward-only reader over a byte buffer.
///
/// Every read first checks that enough bytes remain and returns
/// [`CodecError::ShortBuffer`] otherwise, so the reader can never index out of
/// bounds. It borrows the input rather than copying it.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn is_fully_consumed(&self) -> bool {
        self.pos == self.bytes.len()
    }

    /// Advances over `n` bytes and returns them, or fails if fewer remain.
    fn read_slice(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.remaining() < n {
            return Err(CodecError::ShortBuffer);
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// Reads a fixed-size array. `read_slice` guarantees the source is exactly `N`
    /// bytes, so the `copy_from_slice` cannot panic.
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let slice = self.read_slice(N)?;
        let mut arr = [0u8; N];
        arr.copy_from_slice(slice);
        Ok(arr)
    }

    fn read_u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.read_array::<1>()?[0])
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    fn read_i32(&mut self) -> Result<i32, CodecError> {
        Ok(i32::from_le_bytes(self.read_array::<4>()?))
    }

    fn read_i64(&mut self) -> Result<i64, CodecError> {
        Ok(i64::from_le_bytes(self.read_array::<8>()?))
    }

    fn read_i128(&mut self) -> Result<i128, CodecError> {
        Ok(i128::from_le_bytes(self.read_array::<16>()?))
    }

    fn read_bool(&mut self) -> Result<bool, CodecError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(CodecError::BadBool { value }),
        }
    }

    /// Reads an `Option<T>`: a tag byte then, when `Some`, the inner value.
    fn read_option<T>(
        &mut self,
        read_inner: impl FnOnce(&mut Self) -> Result<T, CodecError>,
    ) -> Result<Option<T>, CodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(read_inner(self)?)),
            value => Err(CodecError::BadBool { value }),
        }
    }

    /// Reads a `String` capped at the global [`MAX_STRING_BYTES`].
    fn read_string(&mut self) -> Result<String, CodecError> {
        self.read_string_capped(MAX_STRING_BYTES)
    }

    /// Reads a `String` whose byte length is capped at a caller-supplied `max`.
    ///
    /// The global [`read_string`](Self::read_string) caps at [`MAX_STRING_BYTES`]
    /// (1024); the v3 manifest needs both larger fields (`description` up to 2000) and
    /// tighter per-field caps, so the cap is a parameter here. An over-cap length
    /// prefix is rejected as [`CodecError::Oversized`] before any bytes are read.
    fn read_string_capped(&mut self, max: usize) -> Result<String, CodecError> {
        let len = self.read_u32()? as usize;
        if len > max {
            return Err(CodecError::Oversized {
                size: len,
                limit: max,
            });
        }
        let raw = self.read_slice(len)?;
        core::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| CodecError::BadUtf8)
    }

    /// Reads a `Decimal` from `(i128 mantissa, i32 scale)`, validating the range so
    /// an out-of-range pair becomes a typed error rather than a panic.
    fn read_decimal(&mut self) -> Result<Decimal, CodecError> {
        let mantissa = self.read_i128()?;
        let scale = self.read_i32()?;
        // A negative scale cannot be a `Decimal` scale (which is `0..=28`); reject it
        // before the (non-panicking) reconstruction is even attempted.
        let scale = u32::try_from(scale).map_err(|_| CodecError::DecimalOutOfRange)?;
        Decimal::try_from_i128_with_scale(mantissa, scale)
            .map_err(|_| CodecError::DecimalOutOfRange)
    }

    /// Reads one [`Candle`] (ABI v2): the `timestamp_ms` then the five OHLCV
    /// decimals, in order. A candle carries no asset of its own; the asset lives once
    /// on the enclosing [`MarketWindow`]. Each `read_decimal` validates its range, so
    /// a malformed candle decimal becomes a typed error rather than a panic.
    fn read_candle(&mut self) -> Result<Candle, CodecError> {
        Ok(Candle {
            timestamp_ms: self.read_i64()?,
            open: self.read_decimal()?,
            high: self.read_decimal()?,
            low: self.read_decimal()?,
            close: self.read_decimal()?,
            volume: self.read_decimal()?,
        })
    }

    fn read_exchange(&mut self) -> Result<Exchange, CodecError> {
        match self.read_u8()? {
            0 => Ok(Exchange::Hyperliquid),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "Exchange",
                value,
            }),
        }
    }

    fn read_product_type(&mut self) -> Result<ProductType, CodecError> {
        match self.read_u8()? {
            0 => Ok(ProductType::Spot),
            1 => Ok(ProductType::Perp),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "ProductType",
                value,
            }),
        }
    }

    fn read_order_side(&mut self) -> Result<OrderSide, CodecError> {
        match self.read_u8()? {
            0 => Ok(OrderSide::Buy),
            1 => Ok(OrderSide::Sell),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "OrderSide",
                value,
            }),
        }
    }

    fn read_position_side(&mut self) -> Result<PositionSide, CodecError> {
        match self.read_u8()? {
            0 => Ok(PositionSide::Long),
            1 => Ok(PositionSide::Short),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "PositionSide",
                value,
            }),
        }
    }

    fn read_order_type(&mut self) -> Result<OrderType, CodecError> {
        match self.read_u8()? {
            0 => Ok(OrderType::Market),
            1 => Ok(OrderType::Limit),
            2 => Ok(OrderType::StopMarket),
            3 => Ok(OrderType::StopLimit),
            4 => Ok(OrderType::TakeProfitMarket),
            5 => Ok(OrderType::TakeProfitLimit),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "OrderType",
                value,
            }),
        }
    }

    fn read_time_in_force(&mut self) -> Result<TimeInForce, CodecError> {
        match self.read_u8()? {
            0 => Ok(TimeInForce::Gtc),
            1 => Ok(TimeInForce::Ioc),
            2 => Ok(TimeInForce::Fok),
            3 => Ok(TimeInForce::Gtx),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "TimeInForce",
                value,
            }),
        }
    }

    fn read_account_status(&mut self) -> Result<AccountStatus, CodecError> {
        match self.read_u8()? {
            0 => Ok(AccountStatus::Evaluation),
            1 => Ok(AccountStatus::Funded),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "AccountStatus",
                value,
            }),
        }
    }

    fn read_drawdown_kind(&mut self) -> Result<DrawdownKind, CodecError> {
        match self.read_u8()? {
            0 => Ok(DrawdownKind::Static),
            1 => Ok(DrawdownKind::Trailing),
            value => Err(CodecError::UnknownDiscriminant {
                kind: "DrawdownKind",
                value,
            }),
        }
    }

    /// Reads one [`DrawdownRule`] (ABI v3): the [`DrawdownKind`] discriminant then the
    /// three resolved decimals `limit`, `floor`, `high_water_mark`, in order. Like a
    /// [`Candle`] it is a nested sub-shape, decoded only inside an [`AccountContext`].
    fn read_drawdown_rule(&mut self) -> Result<DrawdownRule, CodecError> {
        Ok(DrawdownRule {
            kind: self.read_drawdown_kind()?,
            limit: self.read_decimal()?,
            floor: self.read_decimal()?,
            high_water_mark: self.read_decimal()?,
        })
    }
}

// --- Encoder primitives (all little-endian) --------------------------------

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i128(out: &mut Vec<u8>, value: i128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    // String lengths on this boundary are tiny and capped on decode; the cast is
    // exact for every realistic value.
    put_u32(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

fn put_decimal(out: &mut Vec<u8>, value: Decimal) {
    put_i128(out, value.mantissa());
    // `Decimal::scale` is `0..=28`, so the `i32` cast is exact; the wire carries a
    // signed scale because the format is language-agnostic and the decoder validates
    // the range regardless of producer.
    put_i32(out, value.scale() as i32);
}

fn put_candle(out: &mut Vec<u8>, candle: &Candle) {
    put_i64(out, candle.timestamp_ms);
    put_decimal(out, candle.open);
    put_decimal(out, candle.high);
    put_decimal(out, candle.low);
    put_decimal(out, candle.close);
    put_decimal(out, candle.volume);
}

fn put_option_decimal(out: &mut Vec<u8>, value: Option<Decimal>) {
    match value {
        None => out.push(0),
        Some(decimal) => {
            out.push(1);
            put_decimal(out, decimal);
        }
    }
}

fn put_exchange(out: &mut Vec<u8>, value: Exchange) {
    out.push(match value {
        Exchange::Hyperliquid => 0,
    });
}

fn put_product_type(out: &mut Vec<u8>, value: ProductType) {
    out.push(match value {
        ProductType::Spot => 0,
        ProductType::Perp => 1,
    });
}

fn put_order_side(out: &mut Vec<u8>, value: OrderSide) {
    out.push(match value {
        OrderSide::Buy => 0,
        OrderSide::Sell => 1,
    });
}

fn put_position_side(out: &mut Vec<u8>, value: PositionSide) {
    out.push(match value {
        PositionSide::Long => 0,
        PositionSide::Short => 1,
    });
}

fn put_order_type(out: &mut Vec<u8>, value: OrderType) {
    out.push(match value {
        OrderType::Market => 0,
        OrderType::Limit => 1,
        OrderType::StopMarket => 2,
        OrderType::StopLimit => 3,
        OrderType::TakeProfitMarket => 4,
        OrderType::TakeProfitLimit => 5,
    });
}

fn put_time_in_force(out: &mut Vec<u8>, value: TimeInForce) {
    out.push(match value {
        TimeInForce::Gtc => 0,
        TimeInForce::Ioc => 1,
        TimeInForce::Fok => 2,
        TimeInForce::Gtx => 3,
    });
}

fn put_account_status(out: &mut Vec<u8>, value: AccountStatus) {
    out.push(match value {
        AccountStatus::Evaluation => 0,
        AccountStatus::Funded => 1,
    });
}

fn put_drawdown_kind(out: &mut Vec<u8>, value: DrawdownKind) {
    out.push(match value {
        DrawdownKind::Static => 0,
        DrawdownKind::Trailing => 1,
    });
}

fn put_drawdown_rule(out: &mut Vec<u8>, rule: &DrawdownRule) {
    put_drawdown_kind(out, rule.kind);
    put_decimal(out, rule.limit);
    put_decimal(out, rule.floor);
    put_decimal(out, rule.high_water_mark);
}

/// Writes an `Option<[u8; 32]>` image hash: a `0`/`1` tag, then the 32 raw bytes when
/// present. The decode side reads it back via `read_option` and `read_array::<32>()`.
fn put_image_hash(out: &mut Vec<u8>, value: Option<[u8; 32]>) {
    match value {
        None => out.push(0),
        Some(hash) => {
            out.push(1);
            out.extend_from_slice(&hash);
        }
    }
}

/// Runs a top-level decode under the size cap and the full-consumption rule.
///
/// Every public `decode` goes through here so the two whole-message invariants —
/// oversized refusal and no trailing bytes — are enforced in exactly one place.
fn decode_message<T>(
    bytes: &[u8],
    read_body: impl FnOnce(&mut Cursor<'_>) -> Result<T, CodecError>,
) -> Result<T, CodecError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(CodecError::Oversized {
            size: bytes.len(),
            limit: MAX_MESSAGE_BYTES,
        });
    }
    let mut cursor = Cursor::new(bytes);
    let value = read_body(&mut cursor)?;
    if !cursor.is_fully_consumed() {
        return Err(CodecError::TrailingBytes);
    }
    Ok(value)
}

// --- Boundary DTOs ---------------------------------------------------------

/// A single market observation handed to the guest each tick.
///
/// Minimal on purpose: one candle/quote is all a tick needs, and `timestamp_ms` is
/// the *only* clock the guest ever sees (host-injected), keeping the guest
/// deterministic and clockless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketSnapshot {
    /// The asset this observation is for (for example `"BTC"`).
    pub asset: String,
    /// Host-injected observation time, milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
    /// Open price.
    pub open: Decimal,
    /// High price.
    pub high: Decimal,
    /// Low price.
    pub low: Decimal,
    /// Close price.
    pub close: Decimal,
    /// Traded volume over the candle.
    pub volume: Decimal,
}

impl MarketSnapshot {
    /// Encodes this snapshot into the wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_string(&mut out, &self.asset);
        put_i64(&mut out, self.timestamp_ms);
        put_decimal(&mut out, self.open);
        put_decimal(&mut out, self.high);
        put_decimal(&mut out, self.low);
        put_decimal(&mut out, self.close);
        put_decimal(&mut out, self.volume);
        out
    }

    /// Decodes a snapshot from wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for any malformed, oversized, or trailing input.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            Ok(Self {
                asset: cursor.read_string()?,
                timestamp_ms: cursor.read_i64()?,
                open: cursor.read_decimal()?,
                high: cursor.read_decimal()?,
                low: cursor.read_decimal()?,
                close: cursor.read_decimal()?,
                volume: cursor.read_decimal()?,
            })
        })
    }
}

/// One candle in a [`MarketWindow`] (ABI v2).
///
/// It mirrors the OHLCV-plus-`timestamp_ms` fields of a [`MarketSnapshot`] **minus
/// the asset**: the asset is carried once on the enclosing window, not repeated per
/// candle. On the wire a candle is a fixed 108 bytes: `timestamp_ms` (8) plus five
/// `Decimal`s (20 each). Money is the exact `Decimal`, never `f64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candle {
    /// Host-injected candle close time, milliseconds since the Unix epoch. The only
    /// clock the guest ever sees, so a window adds history without adding a clock.
    pub timestamp_ms: i64,
    /// Open price.
    pub open: Decimal,
    /// High price.
    pub high: Decimal,
    /// Low price.
    pub low: Decimal,
    /// Close price.
    pub close: Decimal,
    /// Traded volume over the candle.
    pub volume: Decimal,
}

/// A bounded, time-ordered window of recent candles handed to a v2 guest each tick.
///
/// This is the ABI v2 boundary DTO. It is the asset plus a length-prefixed array of
/// [`Candle`]s, ordered oldest to newest and ending with the latest. A window-aware
/// bot recomputes a multi-candle indicator from this host-supplied history every
/// tick rather than carrying state across ticks, which keeps it stateless and
/// deterministic. The count is capped at [`MAX_CANDLE_COUNT`]; before that many
/// candles exist (live warm-up) the window is naturally shorter and the guest must
/// tolerate it.
///
/// # Examples
///
/// ```
/// use propify_sandbox_abi::{Candle, MarketWindow};
/// use rust_decimal::Decimal;
///
/// let window = MarketWindow {
///     asset: "BTC".to_string(),
///     candles: vec![Candle {
///         timestamp_ms: 1_700_000_000_000,
///         open: Decimal::new(95_000, 0),
///         high: Decimal::new(95_500, 0),
///         low: Decimal::new(94_800, 0),
///         close: Decimal::new(95_200, 0),
///         volume: Decimal::new(1_234, 0),
///     }],
/// };
/// let bytes = window.encode();
/// assert_eq!(MarketWindow::decode(&bytes), Ok(window));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MarketWindow {
    /// The asset every candle in the window is for (for example `"BTC"`).
    pub asset: String,
    /// The candles, oldest to newest, ending with the latest. Length is bounded by
    /// [`MAX_CANDLE_COUNT`].
    pub candles: Vec<Candle>,
}

impl MarketWindow {
    /// Encodes the window into the v2 wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_string(&mut out, &self.asset);
        // The count fits in `u32` for any realistic window; it is capped on decode.
        put_u32(&mut out, self.candles.len() as u32);
        for candle in &self.candles {
            put_candle(&mut out, candle);
        }
        out
    }

    /// Decodes the window from v2 wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for a malformed candle, an over-cap candle count
    /// ([`CodecError::Oversized`] with the [`MAX_CANDLE_COUNT`] limit), an
    /// out-of-range decimal, a short or oversized buffer, or any trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            let asset = cursor.read_string()?;
            let count = cursor.read_u32()? as usize;
            if count > MAX_CANDLE_COUNT {
                return Err(CodecError::Oversized {
                    size: count,
                    limit: MAX_CANDLE_COUNT,
                });
            }
            // Not pre-sized from the untrusted `count`: a short buffer fails fast on
            // the first missing candle instead of letting a huge `count` drive an
            // allocation. This mirrors `StrategyParams::decode`.
            let mut candles = Vec::new();
            for _ in 0..count {
                candles.push(cursor.read_candle()?);
            }
            Ok(Self { asset, candles })
        })
    }
}

/// The read-only strategy parameters handed to the guest each tick.
///
/// A small, count-prefixed list of named decimals. Names and values are the guest's
/// tuning knobs; the host never interprets them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StrategyParams {
    /// The `(name, value)` pairs, in declaration order.
    pub params: Vec<(String, Decimal)>,
}

impl StrategyParams {
    /// Encodes the parameter list into the wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // The count fits in `u32` for any realistic list; it is capped on decode.
        put_u32(&mut out, self.params.len() as u32);
        for (name, value) in &self.params {
            put_string(&mut out, name);
            put_decimal(&mut out, *value);
        }
        out
    }

    /// Decodes the parameter list from wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for a malformed list, an over-cap count, or any
    /// trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            let count = cursor.read_u32()? as usize;
            if count > MAX_PARAM_COUNT {
                return Err(CodecError::Oversized {
                    size: count,
                    limit: MAX_PARAM_COUNT,
                });
            }
            // Not pre-sized from the untrusted `count`: a short buffer fails fast on
            // the first missing pair instead of letting a huge `count` drive an
            // allocation.
            let mut params = Vec::new();
            for _ in 0..count {
                let name = cursor.read_string()?;
                let value = cursor.read_decimal()?;
                params.push((name, value));
            }
            Ok(Self { params })
        })
    }
}

/// This account's own figures, the only account data the guest may read.
///
/// There is deliberately no account id and no other-account data: the guest learns
/// nothing about who it trades for or about any peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountView {
    /// Account equity.
    pub equity: Decimal,
    /// Margin available to open new exposure.
    pub available_margin: Decimal,
    /// Current open exposure.
    pub exposure: Decimal,
    /// Unrealized profit and loss on open positions.
    pub unrealized_pnl: Decimal,
}

impl AccountView {
    /// Encodes the account view into the wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_decimal(&mut out, self.equity);
        put_decimal(&mut out, self.available_margin);
        put_decimal(&mut out, self.exposure);
        put_decimal(&mut out, self.unrealized_pnl);
        out
    }

    /// Decodes the account view from wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for any malformed, oversized, or trailing input.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            Ok(Self {
                equity: cursor.read_decimal()?,
                available_margin: cursor.read_decimal()?,
                exposure: cursor.read_decimal()?,
                unrealized_pnl: cursor.read_decimal()?,
            })
        })
    }
}

/// The intent a guest emits: an `OrderIntent` minus the two fields the guest may
/// not set.
///
/// `intent_id` is omitted because a deterministic, clockless guest cannot mint a
/// ULID (which needs a clock and randomness, both denied), and no account is named
/// because the guest must not choose who it trades for. The host stamps the
/// `intent_id` from its tick context when bridging this body into a full
/// `OrderIntent` (host-side, in `propify-sandbox`), keeping the resulting full
/// intent deterministic given deterministic inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentBody {
    /// Target venue.
    pub exchange: Exchange,
    /// Asset symbol.
    pub asset: String,
    /// Instrument family.
    pub product_type: ProductType,
    /// Book-level direction.
    pub side: OrderSide,
    /// Side of the resulting position.
    pub position_side: PositionSide,
    /// Order type.
    pub order_type: OrderType,
    /// Time-in-force policy.
    pub time_in_force: TimeInForce,
    /// Order size.
    pub quantity: Decimal,
    /// Limit price, when the order type requires one.
    pub price: Option<Decimal>,
    /// Trigger price, when the order type requires one.
    pub trigger_price: Option<Decimal>,
    /// Whether the order may only reduce an existing position.
    pub reduce_only: bool,
}

impl OrderIntentBody {
    /// Encodes the intent body into the wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_exchange(&mut out, self.exchange);
        put_string(&mut out, &self.asset);
        put_product_type(&mut out, self.product_type);
        put_order_side(&mut out, self.side);
        put_position_side(&mut out, self.position_side);
        put_order_type(&mut out, self.order_type);
        put_time_in_force(&mut out, self.time_in_force);
        put_decimal(&mut out, self.quantity);
        put_option_decimal(&mut out, self.price);
        put_option_decimal(&mut out, self.trigger_price);
        put_bool(&mut out, self.reduce_only);
        out
    }

    /// Decodes the intent body from wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for an unknown enum discriminant, a bad bool/tag, an
    /// out-of-range decimal, a short or oversized buffer, or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            Ok(Self {
                exchange: cursor.read_exchange()?,
                asset: cursor.read_string()?,
                product_type: cursor.read_product_type()?,
                side: cursor.read_order_side()?,
                position_side: cursor.read_position_side()?,
                order_type: cursor.read_order_type()?,
                time_in_force: cursor.read_time_in_force()?,
                quantity: cursor.read_decimal()?,
                price: cursor.read_option(Cursor::read_decimal)?,
                trigger_price: cursor.read_option(Cursor::read_decimal)?,
                reduce_only: cursor.read_bool()?,
            })
        })
    }

    /// Boundary sanity check on a decoded intent.
    ///
    /// This is *not* the risk engine; it is the minimal structural guard the host
    /// applies before recording a guest emission, so a nonsensical order (a
    /// non-positive quantity, a negative price) is refused at the boundary. Deeper
    /// per-order-type and per-account rules live in the risk layer.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::IntentRejected`] with a fixed reason when an invariant
    /// is violated.
    pub fn validate(&self) -> Result<(), CodecError> {
        if self.quantity <= Decimal::ZERO {
            return Err(CodecError::IntentRejected {
                reason: "quantity must be strictly positive",
            });
        }
        if self.price.is_some_and(|price| price < Decimal::ZERO) {
            return Err(CodecError::IntentRejected {
                reason: "price must not be negative",
            });
        }
        if self
            .trigger_price
            .is_some_and(|trigger| trigger < Decimal::ZERO)
        {
            return Err(CodecError::IntentRejected {
                reason: "trigger price must not be negative",
            });
        }
        Ok(())
    }
}

/// The resolved drawdown rule inside an [`AccountContext`] (ABI v3).
///
/// A nested sub-shape, like [`Candle`]: it is never a top-level message and is decoded
/// only as part of an [`AccountContext`]. The host computes every figure server-side
/// and ships the resolved snapshot; the guest reads `floor` as the authoritative
/// current line and never tracks the high-water mark itself. For a [`DrawdownKind::Trailing`]
/// account the `floor` already reflects the `high_water_mark`; for a
/// [`DrawdownKind::Static`] account it is the fixed anchor minus the `limit`. On the
/// wire it is the 1-byte kind discriminant followed by three `Decimal`s (61 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawdownRule {
    /// Whether the floor is fixed ([`DrawdownKind::Static`]) or rises with the
    /// high-water mark ([`DrawdownKind::Trailing`]).
    pub kind: DrawdownKind,
    /// The absolute drawdown allowance.
    pub limit: Decimal,
    /// The host-computed current effective floor: the equity level that, if touched,
    /// breaches the drawdown rule. This is the line the bot should act on.
    pub floor: Decimal,
    /// The host-computed high-water mark, for the bot's own display or logic. The
    /// `floor` already bakes it in.
    pub high_water_mark: Decimal,
}

/// The read-only account context handed to a v3 guest each tick.
///
/// Beyond the figures in [`AccountView`], the guest reads the effective account
/// lifecycle status and the resolved rule set (daily-loss floor, drawdown kind and
/// floor, leverage caps, allowed instruments) so a well-behaved bot can shape its own
/// behaviour within those constraints. The in-bot rules are for *adaptation, not
/// trust*: host-side `propify-risk` remains the sole backstop and enforces every limit
/// regardless of what the bot does. The context is produced host-side and is
/// deterministic per tick, so live behaviour reproduces the backtest.
///
/// # Examples
///
/// ```
/// use propify_sandbox_abi::{AccountContext, AccountStatus, DrawdownKind, DrawdownRule};
/// use rust_decimal::Decimal;
///
/// let context = AccountContext {
///     status: AccountStatus::Evaluation,
///     daily_loss_limit: Decimal::new(2_000, 0),
///     daily_loss_floor: Decimal::new(98_000, 0),
///     drawdown: DrawdownRule {
///         kind: DrawdownKind::Trailing,
///         limit: Decimal::new(4_000, 0),
///         floor: Decimal::new(96_000, 0),
///         high_water_mark: Decimal::new(100_000, 0),
///     },
///     default_leverage: Decimal::new(2, 0),
///     leverage_overrides: vec![("BTC".to_string(), Decimal::new(5, 0))],
///     allowed_instruments: vec!["BTC".to_string(), "ETH".to_string()],
/// };
/// let bytes = context.encode();
/// assert_eq!(AccountContext::decode(&bytes), Ok(context));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountContext {
    /// The account lifecycle status (the first axis of the v3 account model).
    pub status: AccountStatus,
    /// The fixed daily loss limit, absolute, in account currency.
    pub daily_loss_limit: Decimal,
    /// The host-computed equity level that, if touched, breaches today (day-start
    /// equity minus the limit). The bot reads this floor directly.
    pub daily_loss_floor: Decimal,
    /// The resolved drawdown rule (kind, limit, current floor, high-water mark).
    pub drawdown: DrawdownRule,
    /// The default maximum leverage.
    pub default_leverage: Decimal,
    /// Per-asset-class leverage caps overriding the default (for example BTC 5x). A
    /// count-prefixed list of `(asset_class, cap)` pairs, mirroring Propr's
    /// effective-leverage shape. Bounded by `MAX_LEVERAGE_OVERRIDE_COUNT`.
    pub leverage_overrides: Vec<(String, Decimal)>,
    /// The permitted asset symbols (the Hyperliquid perpetual futures). A
    /// count-prefixed list bounded by `MAX_ALLOWED_INSTRUMENT_COUNT`.
    pub allowed_instruments: Vec<String>,
}

impl AccountContext {
    /// Encodes the account context into the v3 wire bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_account_status(&mut out, self.status);
        put_decimal(&mut out, self.daily_loss_limit);
        put_decimal(&mut out, self.daily_loss_floor);
        put_drawdown_rule(&mut out, &self.drawdown);
        put_decimal(&mut out, self.default_leverage);
        // Both lists fit in `u32` for any realistic context; each count is capped on
        // decode.
        put_u32(&mut out, self.leverage_overrides.len() as u32);
        for (asset_class, cap) in &self.leverage_overrides {
            put_string(&mut out, asset_class);
            put_decimal(&mut out, *cap);
        }
        put_u32(&mut out, self.allowed_instruments.len() as u32);
        for instrument in &self.allowed_instruments {
            put_string(&mut out, instrument);
        }
        out
    }

    /// Decodes the account context from v3 wire bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for an unknown [`AccountStatus`] or [`DrawdownKind`]
    /// discriminant, an over-cap list count ([`CodecError::Oversized`]), an
    /// over-cap asset-class string, an out-of-range decimal, a short or oversized
    /// buffer, or any trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            let status = cursor.read_account_status()?;
            let daily_loss_limit = cursor.read_decimal()?;
            let daily_loss_floor = cursor.read_decimal()?;
            let drawdown = cursor.read_drawdown_rule()?;
            let default_leverage = cursor.read_decimal()?;

            let override_count = cursor.read_u32()? as usize;
            if override_count > MAX_LEVERAGE_OVERRIDE_COUNT {
                return Err(CodecError::Oversized {
                    size: override_count,
                    limit: MAX_LEVERAGE_OVERRIDE_COUNT,
                });
            }
            // Not pre-sized from the untrusted `count`: a short buffer fails fast on the
            // first missing pair instead of letting a huge `count` drive an allocation.
            let mut leverage_overrides = Vec::new();
            for _ in 0..override_count {
                let asset_class = cursor.read_string_capped(MAX_ASSET_CLASS_BYTES)?;
                let cap = cursor.read_decimal()?;
                leverage_overrides.push((asset_class, cap));
            }

            let instrument_count = cursor.read_u32()? as usize;
            if instrument_count > MAX_ALLOWED_INSTRUMENT_COUNT {
                return Err(CodecError::Oversized {
                    size: instrument_count,
                    limit: MAX_ALLOWED_INSTRUMENT_COUNT,
                });
            }
            let mut allowed_instruments = Vec::new();
            for _ in 0..instrument_count {
                allowed_instruments.push(cursor.read_string()?);
            }

            Ok(Self {
                status,
                daily_loss_limit,
                daily_loss_floor,
                drawdown,
                default_leverage,
                leverage_overrides,
                allowed_instruments,
            })
        })
    }
}

/// The bot's self-declared identity and metadata, embedded in the artifact (ABI v3).
///
/// The SDK encodes this at build time into a `propify_manifest` wasm custom section, so
/// it is hashed together with the code by `ArtifactId = sha256(module bytes)` and
/// cannot be changed without changing the content address. The host extracts and
/// decodes it during the static scan, before instantiating any guest code.
///
/// This codec is deliberately **dumb**: it does a structural decode and enforces a
/// per-field byte cap on each string, and nothing more. The semantic validators —
/// strict semver on `version`, SPDX on `license`, EIP-55 checksum on `author_erc20`,
/// https-URL on `source_repo_url`, syntactic email on `author_email` — and the image
/// checks live marketplace-side, not here. The author fields are self-declared and are
/// not the authenticated submitter.
///
/// # Examples
///
/// ```
/// use propify_sandbox_abi::BotManifest;
///
/// let manifest = BotManifest {
///     name: "DCA Bot".to_string(),
///     description: "Dollar-cost averaging strategy.".to_string(),
///     version: "1.0.0".to_string(),
///     license: "Apache-2.0".to_string(),
///     image_sha256: None,
///     author_name: "Jane Doe".to_string(),
///     author_email: "jane@example.com".to_string(),
///     author_erc20: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
///     source_repo_url: "https://example.com/jane/dca-bot".to_string(),
/// };
/// let bytes = manifest.encode();
/// assert_eq!(BotManifest::decode(&bytes), Ok(manifest));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotManifest {
    /// Human-readable bot name. Capped at 100 bytes on decode.
    pub name: String,
    /// Short description of the bot. Capped at 2000 bytes on decode.
    pub description: String,
    /// The bot version. Semver is validated marketplace-side; capped at 64 bytes here.
    pub version: String,
    /// A single SPDX license identifier. Validated marketplace-side; capped at 64 bytes.
    pub license: String,
    /// Content hash of a separately uploaded image, or `None` when the bot ships none.
    pub image_sha256: Option<[u8; 32]>,
    /// Self-declared author name. Capped at 100 bytes on decode.
    pub author_name: String,
    /// Self-declared author email. Validated syntactically marketplace-side; capped at
    /// 254 bytes here. Not verified.
    pub author_email: String,
    /// Self-declared author ERC20 address. EIP-55 checksum is validated
    /// marketplace-side; capped at 42 bytes here.
    pub author_erc20: String,
    /// Link to the bot source repository. https-only is validated marketplace-side;
    /// capped at 512 bytes here.
    pub source_repo_url: String,
}

impl BotManifest {
    /// Encodes the manifest into the v3 wire bytes for the `propify_manifest` section.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_string(&mut out, &self.name);
        put_string(&mut out, &self.description);
        put_string(&mut out, &self.version);
        put_string(&mut out, &self.license);
        put_image_hash(&mut out, self.image_sha256);
        put_string(&mut out, &self.author_name);
        put_string(&mut out, &self.author_email);
        put_string(&mut out, &self.author_erc20);
        put_string(&mut out, &self.source_repo_url);
        out
    }

    /// Decodes the manifest from the `propify_manifest` section bytes.
    ///
    /// This is a structural decode with per-field byte caps only. Each over-cap string
    /// is rejected as [`CodecError::Oversized`] before its bytes are read. Semantic
    /// validation is the marketplace's job and does not happen here. The overall
    /// [`MAX_MANIFEST_BYTES`] cap is enforced by the scanner before this is called; the
    /// codec's own backstop is the [`MAX_MESSAGE_BYTES`] cap inside [`decode_message`].
    ///
    /// # Errors
    ///
    /// Returns a [`CodecError`] for an over-cap field ([`CodecError::Oversized`]),
    /// invalid UTF-8 in any string, a bad image-hash tag byte, a short or oversized
    /// buffer, or any trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        decode_message(bytes, |cursor| {
            Ok(Self {
                name: cursor.read_string_capped(MAX_MANIFEST_NAME_BYTES)?,
                description: cursor.read_string_capped(MAX_MANIFEST_DESCRIPTION_BYTES)?,
                version: cursor.read_string_capped(MAX_MANIFEST_VERSION_BYTES)?,
                license: cursor.read_string_capped(MAX_MANIFEST_LICENSE_BYTES)?,
                image_sha256: cursor.read_option(|inner| inner.read_array::<32>())?,
                author_name: cursor.read_string_capped(MAX_MANIFEST_AUTHOR_NAME_BYTES)?,
                author_email: cursor.read_string_capped(MAX_MANIFEST_AUTHOR_EMAIL_BYTES)?,
                author_erc20: cursor.read_string_capped(MAX_MANIFEST_AUTHOR_ERC20_BYTES)?,
                source_repo_url: cursor.read_string_capped(MAX_MANIFEST_SOURCE_REPO_URL_BYTES)?,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// A representative, fully-populated intent body for round-trip tests.
    fn sample_body() -> OrderIntentBody {
        OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: "BTC".to_string(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            quantity: dec!(0.5),
            price: Some(dec!(95000.25)),
            trigger_price: None,
            reduce_only: false,
        }
    }

    #[test]
    fn intent_body_round_trips() {
        let body = sample_body();
        let bytes = body.encode();
        assert_eq!(OrderIntentBody::decode(&bytes), Ok(body));
    }

    #[test]
    fn market_snapshot_round_trips() {
        let snapshot = MarketSnapshot {
            asset: "ETH".to_string(),
            timestamp_ms: 1_700_000_000_000,
            open: dec!(3000.10),
            high: dec!(3050.00),
            low: dec!(2990.99),
            close: dec!(3025.55),
            volume: dec!(12345.6789),
        };
        let bytes = snapshot.encode();
        assert_eq!(MarketSnapshot::decode(&bytes), Ok(snapshot));
    }

    #[test]
    fn strategy_params_round_trip_including_empty() {
        let empty = StrategyParams::default();
        assert_eq!(StrategyParams::decode(&empty.encode()), Ok(empty));

        let params = StrategyParams {
            params: vec![
                ("fast".to_string(), dec!(12)),
                ("slow".to_string(), dec!(26)),
                ("threshold".to_string(), dec!(0.0015)),
            ],
        };
        assert_eq!(StrategyParams::decode(&params.encode()), Ok(params));
    }

    /// A representative candle at a given timestamp for window tests.
    fn sample_candle(timestamp_ms: i64) -> Candle {
        Candle {
            timestamp_ms,
            open: dec!(3000.10),
            high: dec!(3050.00),
            low: dec!(2990.99),
            close: dec!(3025.55),
            volume: dec!(12345.6789),
        }
    }

    #[test]
    fn market_window_round_trips_including_empty() {
        // An empty window (live warm-up before any candle exists) round-trips.
        let empty = MarketWindow {
            asset: "BTC".to_string(),
            candles: Vec::new(),
        };
        assert_eq!(MarketWindow::decode(&empty.encode()), Ok(empty));

        // A populated, time-ordered window round-trips byte-for-byte.
        let window = MarketWindow {
            asset: "ETH".to_string(),
            candles: vec![
                sample_candle(1_700_000_000_000),
                sample_candle(1_700_000_060_000),
                sample_candle(1_700_000_120_000),
            ],
        };
        assert_eq!(MarketWindow::decode(&window.encode()), Ok(window));
    }

    #[test]
    fn market_window_at_full_cap_round_trips() {
        // Exactly MAX_CANDLE_COUNT candles is the largest accepted window.
        let candles = (0..MAX_CANDLE_COUNT as i64)
            .map(|i| sample_candle(1_700_000_000_000 + i * 60_000))
            .collect();
        let window = MarketWindow {
            asset: "BTC".to_string(),
            candles,
        };
        assert_eq!(MarketWindow::decode(&window.encode()), Ok(window));
    }

    #[test]
    fn market_window_over_cap_count_is_rejected_as_oversized() {
        // Hand-built bytes claiming one more candle than the cap. The count is
        // refused before any candle is read, so no candle bytes are needed.
        let over = MAX_CANDLE_COUNT + 1;
        let mut bytes = Vec::new();
        put_string(&mut bytes, "BTC");
        put_u32(&mut bytes, over as u32);
        assert_eq!(
            MarketWindow::decode(&bytes),
            Err(CodecError::Oversized {
                size: over,
                limit: MAX_CANDLE_COUNT,
            })
        );
    }

    #[test]
    fn market_window_trailing_bytes_are_rejected() {
        let window = MarketWindow {
            asset: "BTC".to_string(),
            candles: vec![sample_candle(1_700_000_000_000)],
        };
        let mut bytes = window.encode();
        bytes.push(0xff);
        assert_eq!(MarketWindow::decode(&bytes), Err(CodecError::TrailingBytes));
    }

    #[test]
    fn market_window_short_buffer_is_rejected() {
        let window = MarketWindow {
            asset: "BTC".to_string(),
            candles: vec![sample_candle(1_700_000_000_000)],
        };
        let bytes = window.encode();
        // Drop the final bytes so the last candle field runs off the end.
        let truncated = &bytes[..bytes.len() - 3];
        assert_eq!(
            MarketWindow::decode(truncated),
            Err(CodecError::ShortBuffer)
        );
    }

    #[test]
    fn market_window_candle_with_out_of_range_decimal_is_rejected() {
        // One candle whose first (open) decimal has scale 29, one past the maximum.
        // Built by hand so the malformed decimal is exercised through `read_candle`.
        let mut bytes = Vec::new();
        put_string(&mut bytes, "BTC");
        put_u32(&mut bytes, 1);
        put_i64(&mut bytes, 1_700_000_000_000);
        put_i128(&mut bytes, 1);
        put_i32(&mut bytes, 29);
        assert_eq!(
            MarketWindow::decode(&bytes),
            Err(CodecError::DecimalOutOfRange)
        );
    }

    #[test]
    fn market_window_at_cap_stays_under_message_cap_with_documented_headroom() {
        // Worst case: the longest asset string the codec accepts (MAX_STRING_BYTES)
        // plus a full MAX_CANDLE_COUNT window. This documents the headroom under the
        // 64 KiB MAX_MESSAGE_BYTES cap and proves the v2 window can never overflow it.
        let asset = "A".repeat(MAX_STRING_BYTES);
        let candles = (0..MAX_CANDLE_COUNT as i64)
            .map(|i| sample_candle(1_700_000_000_000 + i * 60_000))
            .collect();
        let window = MarketWindow { asset, candles };
        let encoded_len = window.encode().len();

        // Exact worst-case size: 4 (asset len prefix) + 1024 (asset) + 4 (count) +
        // 256 * 108 (candles) = 28_680 bytes.
        const CANDLE_WIRE_BYTES: usize = 108;
        let expected = 4 + MAX_STRING_BYTES + 4 + MAX_CANDLE_COUNT * CANDLE_WIRE_BYTES;
        assert_eq!(
            encoded_len, expected,
            "worst-case window encodes to {expected} bytes"
        );
        assert_eq!(encoded_len, 28_680);

        // Headroom under the 64 KiB message cap: 65_536 - 28_680 = 36_856 bytes.
        assert!(
            encoded_len < MAX_MESSAGE_BYTES,
            "a full v2 window stays under MAX_MESSAGE_BYTES"
        );
        assert_eq!(MAX_MESSAGE_BYTES - encoded_len, 36_856);

        // And it decodes back through the full-consumption, capped decoder.
        assert_eq!(window.encode().len(), encoded_len);
    }

    #[test]
    fn account_view_round_trips() {
        let view = AccountView {
            equity: dec!(100000.00),
            available_margin: dec!(25000.50),
            exposure: dec!(-1500.25),
            unrealized_pnl: dec!(320.75),
        };
        assert_eq!(AccountView::decode(&view.encode()), Ok(view));
    }

    // The (i128 mantissa, i32 scale) decimal carriage is exact for a representative
    // spread of values, with no f64 path anywhere.
    #[test]
    fn decimal_carriage_is_exact_for_representative_values() {
        let values = [
            dec!(0),
            dec!(-1),
            dec!(1),
            dec!(-0.00000001),
            // High scale (28 decimal places, the Decimal maximum).
            Decimal::try_from_i128_with_scale(1, 28).unwrap(),
            // Large magnitude near the 96-bit mantissa bound.
            Decimal::try_from_i128_with_scale(79_228_162_514_264_337_593_543_950_335, 0).unwrap(),
            dec!(-79228162514264337593543950335),
            dec!(123456.789),
        ];
        for value in values {
            // A decimal alone is not a top-level message, so it is wrapped in an
            // account view to exercise the same `read_decimal` path used on the wire.
            let view = AccountView {
                equity: value,
                available_margin: value,
                exposure: value,
                unrealized_pnl: value,
            };
            let decoded = AccountView::decode(&view.encode()).expect("decimal round-trips");
            assert_eq!(decoded.equity, value, "exact value preserved: {value}");
            assert_eq!(decoded, view);
        }
    }

    #[test]
    fn decimal_scale_above_max_is_rejected_not_panicked() {
        // Mantissa 1, scale 29 (one past Decimal::MAX_SCALE). Hand-built wire bytes,
        // padded into an AccountView so the bad first decimal is exercised.
        let mut bytes = Vec::new();
        put_i128(&mut bytes, 1);
        put_i32(&mut bytes, 29);
        for _ in 0..3 {
            put_i128(&mut bytes, 0);
            put_i32(&mut bytes, 0);
        }
        assert_eq!(
            AccountView::decode(&bytes),
            Err(CodecError::DecimalOutOfRange)
        );
    }

    #[test]
    fn decimal_mantissa_beyond_96_bits_is_rejected_not_panicked() {
        // A mantissa larger than the 96-bit magnitude, scale 0.
        let mut bytes = Vec::new();
        put_i128(&mut bytes, i128::MAX);
        put_i32(&mut bytes, 0);
        for _ in 0..3 {
            put_i128(&mut bytes, 0);
            put_i32(&mut bytes, 0);
        }
        assert_eq!(
            AccountView::decode(&bytes),
            Err(CodecError::DecimalOutOfRange)
        );
    }

    #[test]
    fn negative_decimal_scale_is_rejected() {
        let mut bytes = Vec::new();
        put_i128(&mut bytes, 1);
        put_i32(&mut bytes, -1);
        for _ in 0..3 {
            put_i128(&mut bytes, 0);
            put_i32(&mut bytes, 0);
        }
        assert_eq!(
            AccountView::decode(&bytes),
            Err(CodecError::DecimalOutOfRange)
        );
    }

    // Truncated, oversized, and malformed-field inputs each yield a distinct typed
    // error, never a panic and never an OOB read.
    #[test]
    fn truncated_buffer_is_short_buffer() {
        let bytes = sample_body().encode();
        // Drop the final bytes so a field runs off the end.
        let truncated = &bytes[..bytes.len() - 3];
        assert_eq!(
            OrderIntentBody::decode(truncated),
            Err(CodecError::ShortBuffer)
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = sample_body().encode();
        bytes.push(0xff);
        assert_eq!(
            OrderIntentBody::decode(&bytes),
            Err(CodecError::TrailingBytes)
        );
    }

    #[test]
    fn oversized_message_is_rejected() {
        let bytes = vec![0u8; MAX_MESSAGE_BYTES + 1];
        assert_eq!(
            OrderIntentBody::decode(&bytes),
            Err(CodecError::Oversized {
                size: MAX_MESSAGE_BYTES + 1,
                limit: MAX_MESSAGE_BYTES,
            })
        );
    }

    #[test]
    fn unknown_enum_discriminant_is_rejected() {
        let mut bytes = sample_body().encode();
        // First byte is the Exchange discriminant; 9 is not a known variant.
        bytes[0] = 9;
        assert_eq!(
            OrderIntentBody::decode(&bytes),
            Err(CodecError::UnknownDiscriminant {
                kind: "Exchange",
                value: 9,
            })
        );
    }

    #[test]
    fn bad_bool_byte_is_rejected() {
        // reduce_only is the final byte; only 0/1 are valid.
        let mut bytes = sample_body().encode();
        let last = bytes.len() - 1;
        bytes[last] = 7;
        assert_eq!(
            OrderIntentBody::decode(&bytes),
            Err(CodecError::BadBool { value: 7 })
        );
    }

    #[test]
    fn bad_utf8_in_string_is_rejected() {
        // Build a market snapshot manually with an invalid UTF-8 asset.
        let mut bytes = Vec::new();
        put_u32(&mut bytes, 1);
        bytes.push(0xff); // not valid UTF-8
        put_i64(&mut bytes, 0);
        for _ in 0..5 {
            put_decimal(&mut bytes, dec!(0));
        }
        assert_eq!(MarketSnapshot::decode(&bytes), Err(CodecError::BadUtf8));
    }

    #[test]
    fn non_positive_quantity_is_rejected_by_validate() {
        let mut body = sample_body();
        body.quantity = dec!(0);
        assert_eq!(
            body.validate(),
            Err(CodecError::IntentRejected {
                reason: "quantity must be strictly positive",
            })
        );
    }

    #[test]
    fn valid_body_passes_validate() {
        assert_eq!(sample_body().validate(), Ok(()));
    }

    // --- ABI v3: AccountContext ----------------------------------------------

    /// A representative drawdown rule for account-context tests.
    fn sample_drawdown() -> DrawdownRule {
        DrawdownRule {
            kind: DrawdownKind::Trailing,
            limit: dec!(4000),
            floor: dec!(96000.50),
            high_water_mark: dec!(100000),
        }
    }

    /// A representative, fully-populated account context for round-trip tests.
    fn sample_context() -> AccountContext {
        AccountContext {
            status: AccountStatus::Funded,
            daily_loss_limit: dec!(2000),
            daily_loss_floor: dec!(98000.25),
            drawdown: sample_drawdown(),
            default_leverage: dec!(2),
            leverage_overrides: vec![
                ("BTC".to_string(), dec!(5)),
                ("ETH".to_string(), dec!(5)),
                ("crypto".to_string(), dec!(2)),
            ],
            allowed_instruments: vec!["BTC".to_string(), "ETH".to_string(), "SOL".to_string()],
        }
    }

    #[test]
    fn account_context_round_trips_with_overrides_and_instruments() {
        let context = sample_context();
        assert_eq!(AccountContext::decode(&context.encode()), Ok(context));
    }

    #[test]
    fn account_context_round_trips_with_empty_lists() {
        // No leverage overrides and no allowed instruments still round-trips: both
        // count prefixes are zero.
        let context = AccountContext {
            leverage_overrides: Vec::new(),
            allowed_instruments: Vec::new(),
            ..sample_context()
        };
        assert_eq!(AccountContext::decode(&context.encode()), Ok(context));
    }

    #[test]
    fn account_context_at_full_caps_round_trips() {
        // Exactly MAX_LEVERAGE_OVERRIDE_COUNT overrides and
        // MAX_ALLOWED_INSTRUMENT_COUNT instruments is the largest accepted context.
        let leverage_overrides = (0..MAX_LEVERAGE_OVERRIDE_COUNT)
            .map(|i| (format!("class{i}"), dec!(3)))
            .collect();
        let allowed_instruments = (0..MAX_ALLOWED_INSTRUMENT_COUNT)
            .map(|i| format!("INST{i}"))
            .collect();
        let context = AccountContext {
            leverage_overrides,
            allowed_instruments,
            ..sample_context()
        };
        assert_eq!(AccountContext::decode(&context.encode()), Ok(context));
    }

    #[test]
    fn account_context_over_cap_override_count_is_rejected_as_oversized() {
        // Hand-built bytes claiming one more override than the cap. The count is
        // refused before any pair is read, so no pair bytes are needed.
        let over = MAX_LEVERAGE_OVERRIDE_COUNT + 1;
        let mut bytes = Vec::new();
        put_account_status(&mut bytes, AccountStatus::Evaluation);
        put_decimal(&mut bytes, dec!(0));
        put_decimal(&mut bytes, dec!(0));
        put_drawdown_rule(&mut bytes, &sample_drawdown());
        put_decimal(&mut bytes, dec!(0));
        put_u32(&mut bytes, over as u32);
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::Oversized {
                size: over,
                limit: MAX_LEVERAGE_OVERRIDE_COUNT,
            })
        );
    }

    #[test]
    fn account_context_over_cap_instrument_count_is_rejected_as_oversized() {
        // A valid (empty) override list, then an over-cap instrument count.
        let over = MAX_ALLOWED_INSTRUMENT_COUNT + 1;
        let mut bytes = Vec::new();
        put_account_status(&mut bytes, AccountStatus::Evaluation);
        put_decimal(&mut bytes, dec!(0));
        put_decimal(&mut bytes, dec!(0));
        put_drawdown_rule(&mut bytes, &sample_drawdown());
        put_decimal(&mut bytes, dec!(0));
        put_u32(&mut bytes, 0);
        put_u32(&mut bytes, over as u32);
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::Oversized {
                size: over,
                limit: MAX_ALLOWED_INSTRUMENT_COUNT,
            })
        );
    }

    #[test]
    fn account_context_unknown_status_discriminant_is_rejected() {
        let mut bytes = sample_context().encode();
        // The first byte is the AccountStatus discriminant; 9 is not a known variant.
        bytes[0] = 9;
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::UnknownDiscriminant {
                kind: "AccountStatus",
                value: 9,
            })
        );
    }

    #[test]
    fn account_context_unknown_drawdown_kind_discriminant_is_rejected() {
        let mut bytes = sample_context().encode();
        // Layout: status (1) + daily_loss_limit (20) + daily_loss_floor (20) puts the
        // DrawdownKind discriminant at byte 41.
        bytes[41] = 9;
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::UnknownDiscriminant {
                kind: "DrawdownKind",
                value: 9,
            })
        );
    }

    #[test]
    fn account_context_trailing_bytes_are_rejected() {
        let mut bytes = sample_context().encode();
        bytes.push(0xff);
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::TrailingBytes)
        );
    }

    #[test]
    fn account_context_short_buffer_is_rejected() {
        let bytes = sample_context().encode();
        // Drop the final bytes so the last instrument string runs off the end.
        let truncated = &bytes[..bytes.len() - 3];
        assert_eq!(
            AccountContext::decode(truncated),
            Err(CodecError::ShortBuffer)
        );
    }

    #[test]
    fn account_context_over_cap_asset_class_string_is_rejected_as_oversized() {
        // A single override whose asset-class string is one byte over its per-field cap.
        // The tighter `MAX_ASSET_CLASS_BYTES` cap (not the global 1024) must fire, and it
        // fires on the length prefix before any string bytes are read.
        let over = MAX_ASSET_CLASS_BYTES + 1;
        let mut bytes = Vec::new();
        put_account_status(&mut bytes, AccountStatus::Evaluation);
        put_decimal(&mut bytes, dec!(0));
        put_decimal(&mut bytes, dec!(0));
        put_drawdown_rule(&mut bytes, &sample_drawdown());
        put_decimal(&mut bytes, dec!(0));
        put_u32(&mut bytes, 1); // one override follows
        put_string(&mut bytes, &"a".repeat(over));
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::Oversized {
                size: over,
                limit: MAX_ASSET_CLASS_BYTES,
            })
        );
    }

    #[test]
    fn account_context_over_cap_allowed_instrument_string_is_rejected_as_oversized() {
        // An empty override list, then one allowed-instrument string one byte over the
        // global `MAX_STRING_BYTES` cap (the list uses the default `read_string`). The
        // length prefix is refused before any string bytes are read.
        let over = MAX_STRING_BYTES + 1;
        let mut bytes = Vec::new();
        put_account_status(&mut bytes, AccountStatus::Evaluation);
        put_decimal(&mut bytes, dec!(0));
        put_decimal(&mut bytes, dec!(0));
        put_drawdown_rule(&mut bytes, &sample_drawdown());
        put_decimal(&mut bytes, dec!(0));
        put_u32(&mut bytes, 0); // no overrides
        put_u32(&mut bytes, 1); // one instrument follows
        put_string(&mut bytes, &"a".repeat(over));
        assert_eq!(
            AccountContext::decode(&bytes),
            Err(CodecError::Oversized {
                size: over,
                limit: MAX_STRING_BYTES,
            })
        );
    }

    // --- ABI v3: BotManifest -------------------------------------------------

    /// A representative, valid manifest (every field within its cap) for tests.
    fn baseline_manifest() -> BotManifest {
        BotManifest {
            name: "DCA Bot".to_string(),
            description: "Dollar-cost averaging strategy.".to_string(),
            version: "1.0.0".to_string(),
            license: "Apache-2.0".to_string(),
            image_sha256: None,
            author_name: "Jane Doe".to_string(),
            author_email: "jane@example.com".to_string(),
            author_erc20: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            source_repo_url: "https://example.com/jane/dca-bot".to_string(),
        }
    }

    /// A manifest field's name, its byte cap, and a setter for the boundary tests.
    type ManifestFieldCap = (&'static str, usize, fn(&mut BotManifest, String));

    /// The per-field byte caps as `(field name, cap, setter)` for boundary tests.
    fn manifest_field_caps() -> Vec<ManifestFieldCap> {
        vec![
            ("name", MAX_MANIFEST_NAME_BYTES, |m, s| m.name = s),
            ("description", MAX_MANIFEST_DESCRIPTION_BYTES, |m, s| {
                m.description = s
            }),
            ("version", MAX_MANIFEST_VERSION_BYTES, |m, s| m.version = s),
            ("license", MAX_MANIFEST_LICENSE_BYTES, |m, s| m.license = s),
            ("author_name", MAX_MANIFEST_AUTHOR_NAME_BYTES, |m, s| {
                m.author_name = s
            }),
            ("author_email", MAX_MANIFEST_AUTHOR_EMAIL_BYTES, |m, s| {
                m.author_email = s
            }),
            ("author_erc20", MAX_MANIFEST_AUTHOR_ERC20_BYTES, |m, s| {
                m.author_erc20 = s
            }),
            (
                "source_repo_url",
                MAX_MANIFEST_SOURCE_REPO_URL_BYTES,
                |m, s| m.source_repo_url = s,
            ),
        ]
    }

    #[test]
    fn bot_manifest_round_trips_with_image() {
        let manifest = BotManifest {
            image_sha256: Some([7u8; 32]),
            ..baseline_manifest()
        };
        assert_eq!(BotManifest::decode(&manifest.encode()), Ok(manifest));
    }

    #[test]
    fn bot_manifest_round_trips_without_image() {
        let manifest = baseline_manifest();
        assert_eq!(BotManifest::decode(&manifest.encode()), Ok(manifest));
    }

    #[test]
    fn bot_manifest_per_field_byte_caps() {
        for (field, cap, set) in manifest_field_caps() {
            // A field at exactly its byte cap decodes cleanly and round-trips.
            let mut at_cap = baseline_manifest();
            set(&mut at_cap, "a".repeat(cap));
            assert_eq!(
                BotManifest::decode(&at_cap.encode()),
                Ok(at_cap),
                "{field} at its {cap}-byte cap must round-trip"
            );

            // One byte over the cap is refused as Oversized before the bytes are read.
            let mut over_cap = baseline_manifest();
            set(&mut over_cap, "a".repeat(cap + 1));
            assert_eq!(
                BotManifest::decode(&over_cap.encode()),
                Err(CodecError::Oversized {
                    size: cap + 1,
                    limit: cap,
                }),
                "{field} one byte over its cap must be rejected as Oversized"
            );
        }
    }

    #[test]
    fn bot_manifest_trailing_bytes_are_rejected() {
        let mut bytes = baseline_manifest().encode();
        bytes.push(0xff);
        assert_eq!(BotManifest::decode(&bytes), Err(CodecError::TrailingBytes));
    }

    #[test]
    fn bot_manifest_short_buffer_is_rejected() {
        let bytes = baseline_manifest().encode();
        // Drop the final bytes so the last field runs off the end.
        let truncated = &bytes[..bytes.len() - 3];
        assert_eq!(BotManifest::decode(truncated), Err(CodecError::ShortBuffer));
    }

    #[test]
    fn bot_manifest_bad_utf8_in_a_field_is_rejected() {
        // A name field with a one-byte, invalid-UTF-8 value. The decoder reaches name
        // first and rejects it before any later field is read.
        let mut bytes = Vec::new();
        put_u32(&mut bytes, 1);
        bytes.push(0xff); // not valid UTF-8
        assert_eq!(BotManifest::decode(&bytes), Err(CodecError::BadUtf8));
    }
}
