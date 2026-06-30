//! A minimal starting-point bot for the Rust guest SDK.
//!
//! This is the smallest complete bot: it implements the `Bot` trait and registers it
//! with `register_bot!`, which generates the ABI exports. It reads the latest candle
//! and the strategy parameters, then emits one market BUY of a fixed `quantity` (the
//! `"quantity"` parameter if present, else a default of 0.001). It ignores the ABI v2
//! window and the ABI v3 account context, the way any snapshot-only bot does. It is a
//! starting point, not a strategy.
//!
//! It also declares its manifest with `declare_manifest!`, which embeds the
//! `propify_manifest` custom section the host extracts at submission. The encoded bytes
//! come from the package `build.rs`.
//!
//! Build it to a guest module (`minimal_bot.wasm`) with:
//!
//! ```text
//! cargo build --example minimal_bot --target wasm32-unknown-unknown --release
//! ```
//!
//! The example is declared `crate-type = ["cdylib"]` in `Cargo.toml`, so the build
//! emits a real `wasm32` guest module under
//! `target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm`.

use propify_bot_sdk::{
    AccountContext, AccountView, Bot, Exchange, MarketSnapshot, MarketWindow, OrderIntentBody,
    OrderSide, OrderType, PositionSide, ProductType, StrategyParams, TimeInForce, declare_manifest,
    register_bot,
};
use rust_decimal::Decimal;

// The bot is constructed only inside the `register_bot!` expansion, which is gated to
// `target_arch = "wasm32"`. On a host build (clippy `--all-targets`, `cargo test`) that
// expansion is empty, so the struct is genuinely unconstructed there; scope the allow to
// the host target only, where the wasm-only use is invisible.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct MinimalBot;

impl Bot for MinimalBot {
    fn on_tick(
        &mut self,
        market: &MarketSnapshot,
        _window: &MarketWindow,
        params: &StrategyParams,
        _account: &AccountView,
        _context: &AccountContext,
    ) -> Option<OrderIntentBody> {
        // Look up the "quantity" parameter by name; fall back to 0.001 if absent.
        // 0.001 is the exact decimal (mantissa 1, scale 3), never an f64.
        let quantity = params
            .params
            .iter()
            .find(|(name, _)| name == "quantity")
            .map(|(_, value)| *value)
            .unwrap_or_else(|| Decimal::new(1, 3));

        Some(OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: market.asset.clone(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            quantity,
            price: None,
            trigger_price: None,
            reduce_only: false,
        })
    }
}

register_bot!(MinimalBot);
declare_manifest!();
