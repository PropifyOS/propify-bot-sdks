//! `propify-bot-sdk` — the Rust guest SDK for the PropifyOS bot sandbox (spec §7.D).
//!
//! This is the creator-facing toolkit for writing a trading bot in Rust and
//! compiling it to `wasm32-unknown-unknown` for the sandbox host. It tracks the
//! shared ABI codec exactly: it does not define its own wire format or capability
//! list, it re-uses [`propify_sandbox_abi`]. Under ABI v2 it reports
//! `abi_version()` = 2 and reads the multi-candle [`MarketWindow`] through the
//! added `host_read_market_window` capability, while a v1 host/guest keeps running
//! (the host dual-supports v1 and v2). It only makes the guest side ergonomic and
//! safe.
//!
//! # Writing a bot
//!
//! Implement [`Bot`] and register it with [`register_bot!`]:
//!
//! ```ignore
//! use propify_bot_sdk::{
//!     register_bot, AccountView, Bot, Exchange, MarketSnapshot, MarketWindow,
//!     OrderIntentBody, OrderSide, OrderType, PositionSide, ProductType, StrategyParams,
//!     TimeInForce,
//! };
//! use rust_decimal::Decimal;
//!
//! struct MyBot;
//!
//! impl Bot for MyBot {
//!     fn on_tick(
//!         &mut self,
//!         market: &MarketSnapshot,
//!         _window: &MarketWindow,
//!         _params: &StrategyParams,
//!         _account: &AccountView,
//!     ) -> Option<OrderIntentBody> {
//!         Some(OrderIntentBody {
//!             exchange: Exchange::Hyperliquid,
//!             asset: market.asset.clone(),
//!             product_type: ProductType::Perp,
//!             side: OrderSide::Buy,
//!             position_side: PositionSide::Long,
//!             order_type: OrderType::Market,
//!             time_in_force: TimeInForce::Ioc,
//!             quantity: Decimal::new(1, 3),
//!             price: None,
//!             trigger_price: None,
//!             reduce_only: false,
//!         })
//!     }
//! }
//!
//! register_bot!(MyBot);
//! ```
//!
//! The creator writes **zero FFI**: [`register_bot!`] generates the `#[no_mangle]`
//! exports the ABI requires (`abi_version`, `alloc`, `dealloc`, `on_tick`; the
//! linear `memory` is auto-exported by the `cdylib` crate type) and wires `on_tick`
//! to the safe driver below.
//!
//! # How a tick runs
//!
//! On each tick the host re-instantiates the module, so there is **no cross-tick
//! guest state**: the SDK constructs a fresh bot per tick. The driver
//! ([`run_tick`]) reads `market`, the ABI v2 `window`, `params`, and `account`
//! through the host read capabilities using the documented alloc + single-retry
//! protocol, decodes them with the shared codec, calls [`Bot::on_tick`], and — if it
//! returns `Some` — encodes the body and calls `host_emit_intent`. The window is the
//! host-supplied candle history; a simple bot ignores it and reads only the latest
//! candle. Money stays exact (`Decimal` `(i128, i32)`); there is no clock, no RNG, and
//! no `f64` on the path.
//!
//! # Determinism
//!
//! The guest is clockless and zero-authority. The only "clock" it sees is the
//! host-injected `MarketSnapshot::timestamp_ms`. Identical inputs produce identical
//! emitted bytes, so a bot's live behaviour matches its backtest.

mod bot;

#[cfg(target_arch = "wasm32")]
mod ffi;

pub use bot::{Bot, HostBindings, run_tick};

// Re-export the boundary DTOs and enums so a creator has a single import surface and
// never depends on `propify-sandbox-abi` directly. `Candle` and `MarketWindow` are the
// ABI v2 additions a window-aware bot reads.
pub use propify_sandbox_abi::{
    AccountView, Candle, CodecError, Exchange, MarketSnapshot, MarketWindow, OrderIntentBody,
    OrderSide, OrderType, PositionSide, ProductType, StrategyParams, TimeInForce,
};

// The wasm-only allocator exports and the real host binding, surfaced for
// [`register_bot!`] to reference as `$crate::...`. They are meaningless off-target,
// so they are gated to wasm32.
#[cfg(target_arch = "wasm32")]
pub use ffi::{WasmHost, wasm_alloc, wasm_dealloc};

/// The ABI major version this SDK targets, as the `i32` the `abi_version` export must
/// return (wasm has no `u32`; the host reinterprets the bit pattern).
///
/// Sourced from [`propify_sandbox_abi::ABI_VERSION`], the single shared definition, so
/// the SDK can never drift from the value the host negotiates. It now reports `2`
/// (the multi-candle window ABI); the host dual-supports v1 and v2, so a bot built on
/// this SDK is accepted either way. The guest-side window read is a later addition;
/// reporting v2 here does not by itself make a bot read the window.
#[must_use]
pub fn abi_version() -> i32 {
    propify_sandbox_abi::ABI_VERSION as i32
}

/// Generates the ABI exports for a [`Bot`] implementation and wires `on_tick` to
/// the safe driver. The creator writes no FFI.
///
/// Pass a constructor expression that evaluates to your [`Bot`] (for a unit struct,
/// just its name): `register_bot!(MyBot);` or `register_bot!(MyBot::new());`.
///
/// The generated exports are emitted only for `target_arch = "wasm32"` (the sandbox
/// target), so building the crate for the host — for example under
/// `cargo test --workspace` — produces no exports and no FFI. A fresh bot is
/// constructed every tick because the host re-instantiates the module per tick, so
/// the guest holds no state between ticks.
#[macro_export]
macro_rules! register_bot {
    ($bot:expr) => {
        // The exports live in a private module so a single `#[allow(unsafe_code)]`
        // covers the `#[unsafe(no_mangle)]` attributes the ABI requires. The
        // creator's own crate can keep `unsafe_code = "deny"`; the bot code stays
        // safe. `#[unsafe(no_mangle)]` still exports each function at its bare name
        // regardless of the enclosing module.
        #[cfg(target_arch = "wasm32")]
        #[allow(unsafe_code)]
        mod __propify_bot_exports {
            use super::*;

            #[unsafe(no_mangle)]
            pub extern "C" fn abi_version() -> i32 {
                $crate::abi_version()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn alloc(size: i32) -> i32 {
                $crate::wasm_alloc(size)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn dealloc(ptr: i32, size: i32) {
                $crate::wasm_dealloc(ptr, size)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn on_tick() {
                // Fresh per tick: the host re-instantiates the module each tick, so
                // there is no state to carry over.
                let mut bot = $bot;
                $crate::run_tick(&mut bot, &mut $crate::WasmHost);
            }
        }
    };
}
