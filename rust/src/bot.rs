//! The creator-facing safe API and the target-independent tick driver.
//!
//! Nothing here is `unsafe` or wasm-specific: the read/emit flow is written against
//! the [`HostBindings`] trait, so the real wasm bindings (in [`crate::ffi`]) and a
//! test mock are both just implementations. This is what lets the protocol logic —
//! the alloc + single-retry read and the encode + emit — be unit-tested off-target.

use propify_sandbox_abi::{
    AccountContext, AccountView, CodecError, MarketSnapshot, MarketWindow, OrderIntentBody,
    StrategyParams,
};

/// Initial size, in bytes, of the buffer the SDK allocates before a host read.
///
/// A market snapshot, a small parameter list, and an account view all encode well
/// under this, so the common case is a single host call with no retry. When a
/// snapshot is larger, the host returns the required length and the SDK re-allocates
/// once (see [`read_snapshot`]).
const INITIAL_READ_CAPACITY: u32 = 256;

/// A trading bot a creator implements and registers with
/// [`register_bot!`](crate::register_bot).
///
/// On each tick the SDK hands the bot the decoded inputs and asks for at most one
/// order. The implementation must be a **pure, deterministic** function of its
/// inputs (and any state it holds within the tick): no clock, no randomness, no
/// `f64`. The only time the guest ever sees is `MarketSnapshot::timestamp_ms`.
pub trait Bot {
    /// Decide what to do this tick, given a read-only view of the latest candle, the
    /// bounded window of recent candles, the strategy parameters, and this account's
    /// own figures.
    ///
    /// Return `Some(body)` to emit one order, or `None` to do nothing. This is an
    /// [`Option`], **not** a `Vec`, on purpose: the host records only the *first*
    /// accepted intent per tick, so a bot can emit at most one. Returning `Some`
    /// does not guarantee placement — the host still bounds-checks and risk-gates
    /// the emission.
    ///
    /// `window` (ABI v2) is the host-supplied history: the asset plus a time-ordered
    /// array of recent candles, oldest to newest, ending with the latest. A simple bot
    /// ignores it and reads only `market`; a window-aware bot recomputes a multi-candle
    /// indicator from it each tick instead of carrying state across ticks. During live
    /// warm-up, before enough candles exist, the window is shorter (and may be empty);
    /// the SDK passes it through unchanged and the bot decides how to handle a short
    /// window — the SDK does not special-case warm-up.
    ///
    /// `context` (ABI v3) is the read-only account context: the lifecycle status plus the
    /// resolved rule set (daily-loss floor, drawdown kind and floor, leverage caps,
    /// allowed instruments). It is for *adaptation, not trust* — host-side risk remains
    /// the sole backstop and enforces every limit regardless — so a well-behaved bot can
    /// size down near a floor or respect a leverage cap, but cannot exceed a limit by
    /// ignoring it. A bot that does not care about the rules simply ignores it.
    fn on_tick(
        &mut self,
        market: &MarketSnapshot,
        window: &MarketWindow,
        params: &StrategyParams,
        account: &AccountView,
        context: &AccountContext,
    ) -> Option<OrderIntentBody>;
}

/// The raw host capability surface plus the guest's own linear-memory allocator.
///
/// This mirrors the five `propify::host_*` imports and the `alloc`/`dealloc`/memory
/// access a guest performs on its own linear memory. The real implementation
/// ([`crate::ffi::WasmHost`]) is the only place this crate uses `unsafe`; tests
/// supply a safe, `Vec`-backed mock. Creators never see or implement this trait.
///
/// All pointers and lengths are `u32` linear-memory offsets/sizes. The read/emit
/// methods return the host's raw `i32` status (a required length, `0` accepted, or a
/// negative error), matching the wire protocol exactly.
pub trait HostBindings {
    /// `propify::host_read_market_data`.
    fn read_market_data(&mut self, ptr: u32, len: u32) -> i32;
    /// `propify::host_read_market_window` (ABI v2). Mirrors
    /// [`read_market_data`](HostBindings::read_market_data): same `(ptr, len) -> i32`
    /// read protocol, serving the encoded [`MarketWindow`] instead of the snapshot.
    fn read_market_window(&mut self, ptr: u32, len: u32) -> i32;
    /// `propify::host_read_strategy_params`.
    fn read_strategy_params(&mut self, ptr: u32, len: u32) -> i32;
    /// `propify::host_read_account_view`.
    fn read_account_view(&mut self, ptr: u32, len: u32) -> i32;
    /// `propify::host_read_account_context` (ABI v3). Mirrors the other reads: same
    /// `(ptr, len) -> i32` read protocol, serving the encoded [`AccountContext`].
    fn read_account_context(&mut self, ptr: u32, len: u32) -> i32;
    /// `propify::host_emit_intent`.
    fn emit_intent(&mut self, ptr: u32, len: u32) -> i32;
    /// Reserve `size` bytes in the guest's linear memory; returns the offset.
    fn alloc(&mut self, size: u32) -> u32;
    /// Release a buffer previously returned by [`alloc`](HostBindings::alloc).
    fn dealloc(&mut self, ptr: u32, size: u32);
    /// Copy `len` bytes out of guest memory at `ptr`.
    fn load(&self, ptr: u32, len: u32) -> Vec<u8>;
    /// Copy `bytes` into guest memory at `ptr` (which must own at least `bytes.len()`
    /// bytes).
    fn store(&mut self, ptr: u32, bytes: &[u8]);
}

/// Runs one tick: read the five inputs, call the bot, and emit any returned intent.
///
/// Invoked by the `on_tick` export that [`register_bot!`](crate::register_bot)
/// generates. If any read fails (a `-1` host error or a malformed snapshot), the
/// tick does nothing rather than guessing — there is no partial state to leak, since
/// the host re-instantiates the module each tick.
///
/// The ABI v2 [`MarketWindow`] and the ABI v3 [`AccountContext`] are read through the
/// same alloc + single-retry protocol as the other inputs. A short or empty window
/// (live warm-up) decodes fine and is handed to the bot as-is; deciding what to do with
/// a short window or with the resolved rules is the bot's job, not the SDK's.
pub fn run_tick<B: Bot, H: HostBindings>(bot: &mut B, host: &mut H) {
    let Some(market) = read_snapshot(
        host,
        |h, ptr, len| h.read_market_data(ptr, len),
        MarketSnapshot::decode,
        INITIAL_READ_CAPACITY,
    ) else {
        return;
    };
    let Some(window) = read_snapshot(
        host,
        |h, ptr, len| h.read_market_window(ptr, len),
        MarketWindow::decode,
        INITIAL_READ_CAPACITY,
    ) else {
        return;
    };
    let Some(params) = read_snapshot(
        host,
        |h, ptr, len| h.read_strategy_params(ptr, len),
        StrategyParams::decode,
        INITIAL_READ_CAPACITY,
    ) else {
        return;
    };
    let Some(account) = read_snapshot(
        host,
        |h, ptr, len| h.read_account_view(ptr, len),
        AccountView::decode,
        INITIAL_READ_CAPACITY,
    ) else {
        return;
    };
    let Some(context) = read_snapshot(
        host,
        |h, ptr, len| h.read_account_context(ptr, len),
        AccountContext::decode,
        INITIAL_READ_CAPACITY,
    ) else {
        return;
    };

    if let Some(body) = bot.on_tick(&market, &window, &params, &account, &context) {
        emit_intent(host, &body);
    }
}

/// Reads one host snapshot using the documented alloc + single-retry protocol, then
/// decodes it with the shared codec.
///
/// The protocol: allocate a guess buffer and call the host read function. The host
/// returns the snapshot's full length `n`. If the buffer was too small the host
/// wrote nothing and `n > capacity`, so the SDK frees the buffer, re-allocates
/// exactly `n` bytes, and retries once. A negative return is the host's `-1`
/// internal error, handled defensively as "no input this tick" (`None`). Every
/// allocation is paired with exactly one `dealloc`.
fn read_snapshot<H, R, T, D>(
    host: &mut H,
    mut read: R,
    decode: D,
    initial_capacity: u32,
) -> Option<T>
where
    H: HostBindings,
    R: FnMut(&mut H, u32, u32) -> i32,
    D: Fn(&[u8]) -> Result<T, CodecError>,
{
    let mut capacity = initial_capacity.max(1);
    let mut ptr = host.alloc(capacity);

    let first = read(host, ptr, capacity);
    if first < 0 {
        // `-1` internal host error: free the buffer and treat the tick as input-less.
        host.dealloc(ptr, capacity);
        return None;
    }
    let needed = first as u32;

    if needed > capacity {
        // Too-small buffer: the host wrote nothing and returned the required length.
        // Re-allocate exactly that and retry once.
        host.dealloc(ptr, capacity);
        capacity = needed;
        ptr = host.alloc(capacity);
        let retry = read(host, ptr, capacity);
        if retry != needed as i32 {
            // The host's answer changed between calls (it should not): bail safely.
            host.dealloc(ptr, capacity);
            return None;
        }
    }

    let bytes = host.load(ptr, needed);
    host.dealloc(ptr, capacity);
    decode(&bytes).ok()
}

/// Encodes a body and offers it to the host via `host_emit_intent`.
///
/// The host records only the first accepted emission per tick and returns a status
/// the guest cannot act on usefully (the order is already decided host-side), so the
/// status is intentionally ignored. The buffer is freed afterwards.
fn emit_intent<H: HostBindings>(host: &mut H, body: &OrderIntentBody) {
    let bytes = body.encode();
    let len = bytes.len() as u32;
    let ptr = host.alloc(len);
    host.store(ptr, &bytes);
    let _status = host.emit_intent(ptr, len);
    host.dealloc(ptr, len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use propify_sandbox_abi::{
        AccountStatus, Candle, DrawdownKind, DrawdownRule, Exchange, OrderSide, OrderType,
        PositionSide, ProductType, TimeInForce,
    };
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    /// A `Vec`-backed mock that mimics the D-2 host read/emit protocol byte-for-byte,
    /// including the too-small-buffer retry and the `-1` internal-error path.
    ///
    /// `memory` simulates the guest's linear memory; `alloc` bump-allocates offsets
    /// into it, exactly as a real allocator hands out addresses the host then writes.
    struct MockHost {
        memory: Vec<u8>,
        next_offset: u32,
        market: Vec<u8>,
        window: Vec<u8>,
        params: Vec<u8>,
        account: Vec<u8>,
        context: Vec<u8>,
        emitted: Option<Vec<u8>>,
        market_read_calls: u32,
        window_read_calls: u32,
        context_read_calls: u32,
        force_market_error: bool,
        force_window_error: bool,
        force_context_error: bool,
    }

    impl MockHost {
        fn new(market: Vec<u8>, params: Vec<u8>, account: Vec<u8>) -> Self {
            Self {
                memory: Vec::new(),
                // Start above 0 so a returned offset is never the null-like 0.
                next_offset: 8,
                market,
                // Default to an encoded empty window, which decodes fine: a tick that
                // does not care about the window still has a valid one to read, and the
                // common case mirrors a v2 host that has no history to serve yet.
                window: MarketWindow::default().encode(),
                params,
                account,
                // Default to a valid, fully-populated account context so a tick that does
                // not care about the rules still has one to read, mirroring a v3 host
                // that always resolves and serves the context.
                context: sample_context().encode(),
                emitted: None,
                market_read_calls: 0,
                window_read_calls: 0,
                context_read_calls: 0,
                force_market_error: false,
                force_window_error: false,
                force_context_error: false,
            }
        }

        /// Sets the encoded [`MarketWindow`] the mock serves for the window read.
        fn with_window(mut self, window: Vec<u8>) -> Self {
            self.window = window;
            self
        }

        /// Sets the encoded [`AccountContext`] the mock serves for the context read.
        fn with_context(mut self, context: Vec<u8>) -> Self {
            self.context = context;
            self
        }
    }

    /// The host side of a read: write the payload into guest memory if it fits,
    /// otherwise write nothing and return the required length. Mirrors the host's
    /// `write_to_guest`.
    fn serve(payload: &[u8], memory: &mut Vec<u8>, ptr: u32, len: u32) -> i32 {
        let needed = payload.len();
        if (len as usize) < needed {
            return needed as i32;
        }
        let start = ptr as usize;
        let end = start + needed;
        if memory.len() < end {
            memory.resize(end, 0);
        }
        memory[start..end].copy_from_slice(payload);
        needed as i32
    }

    impl HostBindings for MockHost {
        fn read_market_data(&mut self, ptr: u32, len: u32) -> i32 {
            self.market_read_calls += 1;
            if self.force_market_error {
                return -1;
            }
            serve(&self.market, &mut self.memory, ptr, len)
        }

        fn read_market_window(&mut self, ptr: u32, len: u32) -> i32 {
            self.window_read_calls += 1;
            if self.force_window_error {
                return -1;
            }
            serve(&self.window, &mut self.memory, ptr, len)
        }

        fn read_strategy_params(&mut self, ptr: u32, len: u32) -> i32 {
            serve(&self.params, &mut self.memory, ptr, len)
        }

        fn read_account_view(&mut self, ptr: u32, len: u32) -> i32 {
            serve(&self.account, &mut self.memory, ptr, len)
        }

        fn read_account_context(&mut self, ptr: u32, len: u32) -> i32 {
            self.context_read_calls += 1;
            if self.force_context_error {
                return -1;
            }
            serve(&self.context, &mut self.memory, ptr, len)
        }

        fn emit_intent(&mut self, ptr: u32, len: u32) -> i32 {
            let start = ptr as usize;
            let end = start + len as usize;
            let bytes = self.memory[start..end].to_vec();
            match OrderIntentBody::decode(&bytes) {
                Ok(body) => match body.validate() {
                    Ok(()) => {
                        // First accepted emission wins, exactly like the host.
                        if self.emitted.is_none() {
                            self.emitted = Some(bytes);
                        }
                        0
                    }
                    Err(_) => -3,
                },
                Err(_) => -2,
            }
        }

        fn alloc(&mut self, size: u32) -> u32 {
            let ptr = self.next_offset;
            let end = ptr as usize + size as usize;
            if self.memory.len() < end {
                self.memory.resize(end, 0);
            }
            self.next_offset = end as u32;
            ptr
        }

        fn dealloc(&mut self, _ptr: u32, _size: u32) {
            // A bump allocator does not reclaim; the mock memory is short-lived.
        }

        fn load(&self, ptr: u32, len: u32) -> Vec<u8> {
            let start = ptr as usize;
            self.memory[start..start + len as usize].to_vec()
        }

        fn store(&mut self, ptr: u32, bytes: &[u8]) {
            let start = ptr as usize;
            let end = start + bytes.len();
            if self.memory.len() < end {
                self.memory.resize(end, 0);
            }
            self.memory[start..end].copy_from_slice(bytes);
        }
    }

    fn sample_market() -> MarketSnapshot {
        MarketSnapshot {
            asset: "BTC".to_string(),
            timestamp_ms: 1_700_000_000_000,
            open: dec!(95000.00),
            high: dec!(95500.00),
            low: dec!(94800.00),
            close: dec!(95200.50),
            volume: dec!(1234.5),
        }
    }

    fn sample_params() -> StrategyParams {
        StrategyParams {
            params: vec![("quantity".to_string(), dec!(0.002))],
        }
    }

    fn sample_account() -> AccountView {
        AccountView {
            equity: dec!(100000.00),
            available_margin: dec!(50000.00),
            exposure: dec!(0),
            unrealized_pnl: dec!(0),
        }
    }

    /// A two-candle window whose latest candle has a distinctive `close`, so a bot
    /// echoing that `close` proves the window was delivered and decoded.
    fn sample_window() -> MarketWindow {
        MarketWindow {
            asset: "BTC".to_string(),
            candles: vec![
                Candle {
                    timestamp_ms: 1_699_999_940_000,
                    open: dec!(94000.00),
                    high: dec!(94500.00),
                    low: dec!(93800.00),
                    close: dec!(94200.00),
                    volume: dec!(1000.0),
                },
                Candle {
                    timestamp_ms: 1_700_000_000_000,
                    open: dec!(95000.00),
                    high: dec!(95500.00),
                    low: dec!(94800.00),
                    close: dec!(95200.50),
                    volume: dec!(1234.5),
                },
            ],
        }
    }

    /// A representative, fully-populated account context the mock serves by default and
    /// the context-aware bot reads. The drawdown floor is the distinctive figure a bot
    /// can echo to prove the context reached it decoded.
    fn sample_context() -> AccountContext {
        AccountContext {
            status: AccountStatus::Funded,
            daily_loss_limit: dec!(2000),
            daily_loss_floor: dec!(98000.25),
            drawdown: DrawdownRule {
                kind: DrawdownKind::Trailing,
                limit: dec!(4000),
                floor: dec!(96000.50),
                high_water_mark: dec!(100000),
            },
            default_leverage: dec!(2),
            leverage_overrides: vec![("BTC".to_string(), dec!(5)), ("ETH".to_string(), dec!(5))],
            allowed_instruments: vec!["BTC".to_string(), "ETH".to_string()],
            // Funded accounts carry no profit target.
            profit_target: None,
        }
    }

    /// A bot that echoes the market asset and the `"quantity"` param into a market
    /// BUY, ignoring the window, so a passing assertion proves the snapshot path still
    /// works for a simple (window-unaware) bot.
    struct EchoBot;

    impl Bot for EchoBot {
        fn on_tick(
            &mut self,
            market: &MarketSnapshot,
            _window: &MarketWindow,
            params: &StrategyParams,
            _account: &AccountView,
            _context: &AccountContext,
        ) -> Option<OrderIntentBody> {
            let quantity = params
                .params
                .iter()
                .find(|(name, _)| name == "quantity")
                .map(|(_, value)| *value)
                .unwrap_or(Decimal::ONE);
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

    /// A window-aware bot: it sizes its order by the `close` of the latest candle in
    /// the window, so a passing assertion proves the window reached the bot decoded.
    /// An empty window yields no order, exercising the warm-up path the bot owns.
    struct WindowBot;

    impl Bot for WindowBot {
        fn on_tick(
            &mut self,
            market: &MarketSnapshot,
            window: &MarketWindow,
            _params: &StrategyParams,
            _account: &AccountView,
            _context: &AccountContext,
        ) -> Option<OrderIntentBody> {
            let latest = window.candles.last()?;
            Some(OrderIntentBody {
                exchange: Exchange::Hyperliquid,
                asset: market.asset.clone(),
                product_type: ProductType::Perp,
                side: OrderSide::Buy,
                position_side: PositionSide::Long,
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                quantity: latest.close,
                price: None,
                trigger_price: None,
                reduce_only: false,
            })
        }
    }

    struct SilentBot;

    impl Bot for SilentBot {
        fn on_tick(
            &mut self,
            _market: &MarketSnapshot,
            _window: &MarketWindow,
            _params: &StrategyParams,
            _account: &AccountView,
            _context: &AccountContext,
        ) -> Option<OrderIntentBody> {
            None
        }
    }

    /// A context-aware bot: it sizes its order by the drawdown `floor` from the account
    /// context, so a passing assertion proves the ABI v3 context reached the bot decoded.
    struct ContextBot;

    impl Bot for ContextBot {
        fn on_tick(
            &mut self,
            market: &MarketSnapshot,
            _window: &MarketWindow,
            _params: &StrategyParams,
            _account: &AccountView,
            context: &AccountContext,
        ) -> Option<OrderIntentBody> {
            Some(OrderIntentBody {
                exchange: Exchange::Hyperliquid,
                asset: market.asset.clone(),
                product_type: ProductType::Perp,
                side: OrderSide::Buy,
                position_side: PositionSide::Long,
                order_type: OrderType::Market,
                time_in_force: TimeInForce::Ioc,
                quantity: context.drawdown.floor,
                price: None,
                trigger_price: None,
                reduce_only: false,
            })
        }
    }

    #[test]
    fn run_tick_serves_a_window_yet_a_snapshot_only_bot_still_emits() {
        // `EchoBot` ignores the window. The mock still serves one (the default empty
        // window), the driver still reads it, and the simple bot emits exactly as
        // before: proving the v2 window read does not disturb a window-unaware bot.
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        );
        run_tick(&mut EchoBot, &mut host);

        let emitted = host.emitted.expect("a bot returning Some must emit");
        // The bytes the SDK encoded must decode back through the shared codec to the
        // exact body the bot built from the delivered inputs (byte agreement with the
        // host codec).
        let expected = OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: "BTC".to_string(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            quantity: dec!(0.002),
            price: None,
            trigger_price: None,
            reduce_only: false,
        };
        assert_eq!(emitted, expected.encode(), "emitted wire bytes must match");
        assert_eq!(
            OrderIntentBody::decode(&emitted),
            Ok(expected),
            "emitted bytes must decode to the expected body"
        );
        // Both reads fit the initial guess, so there was no retry on either.
        assert_eq!(
            host.market_read_calls, 1,
            "no retry expected for a small market read"
        );
        assert_eq!(
            host.window_read_calls, 1,
            "the window is read once per tick, with no retry for a small window"
        );
    }

    #[test]
    fn run_tick_reads_all_four_inputs_and_emits_a_decodable_intent() {
        // `WindowBot` sizes its order by the latest candle's `close`, so a match
        // proves all four inputs — market, window, params, account — reached the bot
        // decoded, and the emitted bytes round-trip through the shared codec.
        let window = sample_window();
        let latest_close = window.candles.last().expect("two-candle window").close;
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        )
        .with_window(window.encode());

        run_tick(&mut WindowBot, &mut host);

        let emitted = host.emitted.expect("a bot returning Some must emit");
        let expected = OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: "BTC".to_string(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            quantity: latest_close,
            price: None,
            trigger_price: None,
            reduce_only: false,
        };
        assert_eq!(
            emitted,
            expected.encode(),
            "the order must be sized by the window's latest candle close"
        );
        assert_eq!(
            OrderIntentBody::decode(&emitted),
            Ok(expected),
            "emitted bytes must decode to the expected body"
        );
        assert_eq!(
            host.window_read_calls, 1,
            "the window is read once per tick"
        );
    }

    #[test]
    fn run_tick_emits_nothing_when_the_bot_returns_none() {
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        );
        run_tick(&mut SilentBot, &mut host);
        assert_eq!(host.emitted, None, "a bot returning None must emit nothing");
    }

    #[test]
    fn read_snapshot_retries_once_on_a_too_small_buffer() {
        // The market payload is far larger than the tiny initial capacity, so the
        // first read returns the required length (writing nothing) and the SDK must
        // re-allocate and retry to get the bytes.
        let market = sample_market();
        let mut host = MockHost::new(market.encode(), Vec::new(), Vec::new());

        let decoded: Option<MarketSnapshot> = read_snapshot(
            &mut host,
            |h, ptr, len| h.read_market_data(ptr, len),
            MarketSnapshot::decode,
            4, // deliberately smaller than the encoded snapshot
        );

        assert_eq!(
            decoded,
            Some(market),
            "retry path must recover the snapshot"
        );
        assert_eq!(
            host.market_read_calls, 2,
            "a too-small buffer must trigger exactly one retry"
        );
    }

    #[test]
    fn read_snapshot_returns_none_on_an_internal_host_error() {
        let mut host = MockHost::new(sample_market().encode(), Vec::new(), Vec::new());
        host.force_market_error = true;

        let decoded: Option<MarketSnapshot> = read_snapshot(
            &mut host,
            |h, ptr, len| h.read_market_data(ptr, len),
            MarketSnapshot::decode,
            INITIAL_READ_CAPACITY,
        );
        assert_eq!(decoded, None, "a -1 host error must yield no snapshot");
    }

    #[test]
    fn read_snapshot_retries_once_on_a_too_small_window_buffer() {
        // The encoded window is larger than the tiny initial capacity, so the first
        // window read returns the required length (writing nothing) and the SDK must
        // re-allocate and retry to get the bytes — the same protocol as every read.
        let window = sample_window();
        let mut host =
            MockHost::new(Vec::new(), Vec::new(), Vec::new()).with_window(window.encode());

        let decoded: Option<MarketWindow> = read_snapshot(
            &mut host,
            |h, ptr, len| h.read_market_window(ptr, len),
            MarketWindow::decode,
            4, // deliberately smaller than the encoded window
        );

        assert_eq!(decoded, Some(window), "retry path must recover the window");
        assert_eq!(
            host.window_read_calls, 2,
            "a too-small window buffer must trigger exactly one retry"
        );
    }

    #[test]
    fn run_tick_does_nothing_when_the_window_read_errors() {
        // A `-1` internal host error on the window read aborts the tick with no
        // emission, exactly as a failed market read does: no input is guessed.
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        )
        .with_window(sample_window().encode());
        host.force_window_error = true;

        run_tick(&mut WindowBot, &mut host);
        assert_eq!(
            host.emitted, None,
            "a failed window read must abort the tick with no emission"
        );
    }

    #[test]
    fn run_tick_does_nothing_when_a_read_errors() {
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        );
        host.force_market_error = true;
        run_tick(&mut EchoBot, &mut host);
        assert_eq!(
            host.emitted, None,
            "a failed read must abort the tick with no emission"
        );
    }

    #[test]
    fn run_tick_delivers_the_account_context_decoded_to_the_bot() {
        // `ContextBot` sizes its order by the context's drawdown floor, so a match proves
        // the ABI v3 account context — read through the same protocol as the other
        // inputs — reached the bot decoded, and the emitted bytes round-trip.
        let context = sample_context();
        let floor = context.drawdown.floor;
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        )
        .with_context(context.encode());

        run_tick(&mut ContextBot, &mut host);

        let emitted = host.emitted.expect("a bot returning Some must emit");
        let expected = OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: "BTC".to_string(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            quantity: floor,
            price: None,
            trigger_price: None,
            reduce_only: false,
        };
        assert_eq!(
            emitted,
            expected.encode(),
            "the order must be sized by the context's drawdown floor"
        );
        assert_eq!(
            OrderIntentBody::decode(&emitted),
            Ok(expected),
            "emitted bytes must decode to the expected body"
        );
        assert_eq!(
            host.context_read_calls, 1,
            "the account context is read once per tick"
        );
    }

    #[test]
    fn run_tick_does_nothing_when_the_context_read_errors() {
        // A `-1` internal host error on the context read aborts the tick with no
        // emission, exactly as a failed market or window read does: no input is guessed.
        let mut host = MockHost::new(
            sample_market().encode(),
            sample_params().encode(),
            sample_account().encode(),
        );
        host.force_context_error = true;

        run_tick(&mut ContextBot, &mut host);
        assert_eq!(
            host.emitted, None,
            "a failed context read must abort the tick with no emission"
        );
    }
}
