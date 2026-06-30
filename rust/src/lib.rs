//! `propify-bot-sdk` — the Rust guest SDK for the PropifyOS bot sandbox (spec §7.D).
//!
//! This is the creator-facing toolkit for writing a trading bot in Rust and
//! compiling it to `wasm32-unknown-unknown` for the sandbox host. It tracks the
//! shared ABI codec exactly: it does not define its own wire format or capability
//! list, it re-uses [`propify_sandbox_abi`]. It targets ABI v3: it reports
//! `abi_version()` = 3, reads the multi-candle [`MarketWindow`] (added in v2) and the
//! read-only [`AccountContext`] (added in v3) alongside the snapshot, and embeds the
//! bot's [`declare_manifest!`] section. v3 is the single supported version. The SDK
//! only makes the guest side ergonomic and safe.
//!
//! # Writing a bot
//!
//! Implement [`Bot`] and register it with [`register_bot!`]:
//!
//! ```ignore
//! use propify_bot_sdk::{
//!     register_bot, AccountContext, AccountView, Bot, Exchange, MarketSnapshot,
//!     MarketWindow, OrderIntentBody, OrderSide, OrderType, PositionSide, ProductType,
//!     StrategyParams, TimeInForce,
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
//!         _context: &AccountContext,
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
//! to the safe driver below. To embed the bot manifest, the bot crate also adds a small
//! build script and one [`declare_manifest!`] line; see that macro's docs.
//!
//! # How a tick runs
//!
//! On each tick the host re-instantiates the module, so there is **no cross-tick
//! guest state**: the SDK constructs a fresh bot per tick. The driver
//! ([`run_tick`]) reads `market`, the ABI v2 `window`, `params`, `account`, and the
//! ABI v3 `context` through the host read capabilities using the documented alloc +
//! single-retry protocol, decodes them with the shared codec, calls [`Bot::on_tick`],
//! and — if it returns `Some` — encodes the body and calls `host_emit_intent`. The
//! window is the host-supplied candle history and the context is the resolved account
//! rules; a simple bot ignores both and reads only the latest candle. Money stays exact
//! (`Decimal` `(i128, i32)`); there is no clock, no RNG, and no `f64` on the path.
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
// ABI v2 additions a window-aware bot reads; `AccountContext`, `AccountStatus`,
// `DrawdownKind`, and `DrawdownRule` are the ABI v3 additions a context-aware bot reads.
pub use propify_sandbox_abi::{
    AccountContext, AccountStatus, AccountView, Candle, CodecError, DrawdownKind, DrawdownRule,
    Exchange, MarketSnapshot, MarketWindow, OrderIntentBody, OrderSide, OrderType, PositionSide,
    ProductType, StrategyParams, TimeInForce,
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
/// the SDK can never drift from the value the host negotiates. It now reports `3`, the
/// single supported version: the host accepts only guests reporting `3` (the v1/v2
/// dual-support path is dropped). v3 adds the read-only [`AccountContext`] and the
/// embedded manifest section; declaring or reading those is per-bot, but the reported
/// version is always `3`.
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

/// Embeds the bot manifest into the `propify_manifest` wasm custom section.
///
/// ABI v3 carries the bot's identity and metadata ([`BotManifest`]) inside the artifact
/// as a custom section, so it is hashed together with the code by
/// `ArtifactId = sha256(module bytes)` and the host can extract and validate it with a
/// pure static parse before running anything. In Rust the toolchain emits a custom
/// section natively from a `#[link_section]` static, so no post-build `wasm-tools`
/// injection is needed (unlike the AssemblyScript and TinyGo SDKs).
///
/// This macro expands to that `#[used] #[link_section = "propify_manifest"]` static. The
/// `#[unsafe(link_section = ...)]` form is the edition-2024 attribute-safety wrapper, not
/// an `unsafe` block, so a bot crate can keep `unsafe_code = "forbid"` and still embed
/// the section. It is gated to `target_arch = "wasm32"`: the section only matters in the
/// guest artifact, and gating it keeps a host build (for example `cargo test`) free of an
/// unused static.
///
/// # The build script that produces the bytes
///
/// The manifest must be the canonical [`BotManifest::encode`] bytes, so the bot crate's
/// build script builds the manifest and writes the encoded bytes to
/// `$OUT_DIR/propify_manifest.bin`, which this macro embeds via `include_bytes!`. Add
/// `propify-sandbox-abi` as a `[build-dependencies]` entry and a `build.rs` like:
///
/// ```ignore
/// // build.rs
/// use std::{env, fs, path::Path};
/// use propify_sandbox_abi::BotManifest;
///
/// fn main() {
///     let manifest = BotManifest {
///         name: "My Bot".to_string(),
///         description: "What it does.".to_string(),
///         version: "0.1.0".to_string(),
///         license: "Apache-2.0".to_string(),
///         image_sha256: None,
///         author_name: "Jane Doe".to_string(),
///         author_email: "jane@example.com".to_string(),
///         author_erc20: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
///         source_repo_url: "https://example.com/jane/my-bot".to_string(),
///     };
///     let out = Path::new(&env::var("OUT_DIR").unwrap()).join("propify_manifest.bin");
///     fs::write(out, manifest.encode()).unwrap();
///     println!("cargo:rerun-if-changed=build.rs");
/// }
/// ```
///
/// Then, once per bot crate, alongside `register_bot!`:
///
/// ```ignore
/// propify_bot_sdk::declare_manifest!();
/// ```
///
/// [`BotManifest`]: propify_sandbox_abi::BotManifest
/// [`BotManifest::encode`]: propify_sandbox_abi::BotManifest::encode
#[macro_export]
macro_rules! declare_manifest {
    () => {
        // The encoded manifest length is a const taken from the build-script output, so
        // the bot author never has to hand-count the byte length for the array type.
        // `include_bytes!` yields a `&[u8; N]` whose `len()` is a const expression.
        #[cfg(target_arch = "wasm32")]
        const PROPIFY_MANIFEST_LEN: usize =
            include_bytes!(concat!(env!("OUT_DIR"), "/propify_manifest.bin")).len();

        // The section bytes themselves. `#[used]` keeps the linker from dropping a static
        // nothing references; `#[link_section]` places it in the `propify_manifest`
        // custom section the host extracts. The `unsafe(...)` is the attribute-safety
        // wrapper required in edition 2024, not an `unsafe` block.
        #[cfg(target_arch = "wasm32")]
        #[used]
        #[unsafe(link_section = "propify_manifest")]
        static PROPIFY_MANIFEST: [u8; PROPIFY_MANIFEST_LEN] =
            *include_bytes!(concat!(env!("OUT_DIR"), "/propify_manifest.bin"));
    };
}
