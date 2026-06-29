// The creator-facing `Bot` abstraction and the tick driver, mirroring the Rust SDK's
// `trait Bot` + `run_tick` and the AssemblyScript `Bot` + `runTick`.
//
// A creator writes a Go struct that implements `Bot.OnTick`; the sample's `main`
// package wires the four ABI exports to the helpers here (Go has no macro system, so
// the wiring is a handful of one-line exported functions instead of Rust's
// `register_bot!`). The driver performs the read -> OnTick -> emit plumbing against the
// host imports (four under v1, five under v2 with the window read), exactly like the
// Rust `run_tick` does against its `HostBindings`.
package propify

// initialReadCapacity is the initial size, in bytes, of the buffer the SDK allocates
// before a host read. A market snapshot, a small parameter list, and an account view
// all encode well under this, so the common case is a single host call with no retry.
const initialReadCapacity int32 = 256

// Bot is a trading bot a creator implements. On each tick the driver hands it the
// decoded inputs and asks for at most one order.
//
// The implementation must be a pure, deterministic function of its inputs: no clock,
// no randomness, no float64. Return an *OrderIntentBody to emit one order, or nil to do
// nothing (mirroring the Rust Option<OrderIntentBody>). Returning a body does not
// guarantee placement — the host still bounds-checks and risk-gates it.
//
// window (ABI v2) is the host-supplied candle history: the asset plus a time-ordered
// slice of recent candles, oldest to newest, ending with the latest. A snapshot-only bot
// ignores it and reads only market; a window-aware bot recomputes a multi-candle
// indicator from it each tick. During live warm-up the window is shorter (and may be
// empty); the SDK hands it through as-is and the bot decides how to handle a short window
// — the SDK does not special-case warm-up.
type Bot interface {
	OnTick(market *MarketSnapshot, window *MarketWindow, params *StrategyParams, account *AccountView) *OrderIntentBody
}

// The host read imports cannot be taken as values directly (a `//go:wasmimport`
// function is not addressable as a value), so each is wrapped in a plain top-level
// function. Passing a top-level function by name yields a static function value with no
// heap boxing, unlike an inline closure.
func readMarketData(ptr, length int32) int32     { return hostReadMarketData(ptr, length) }
func readMarketWindow(ptr, length int32) int32   { return hostReadMarketWindow(ptr, length) }
func readStrategyParams(ptr, length int32) int32 { return hostReadStrategyParams(ptr, length) }
func readAccountView(ptr, length int32) int32    { return hostReadAccountView(ptr, length) }

// readSnapshot reads one host snapshot using the documented alloc + single-retry
// protocol.
//
// It allocates a guess buffer and calls the host. The host returns the snapshot's full
// length n. If the buffer was too small the host wrote nothing and n > capacity, so the
// SDK allocates exactly n and retries once. A negative return is the host's internal
// error, surfaced as failed == true ("no input this tick"). It returns the live bytes
// (sliced to the exact length) on success.
func readSnapshot(read func(ptr int32, length int32) int32, initialCapacity int32) ([]byte, bool) {
	capacity := initialCapacity
	if capacity <= 0 {
		capacity = 1
	}
	ptr, buf := allocBuffer(capacity)

	first := read(int32(ptr), capacity)
	if first < 0 {
		// Internal host error: treat the tick as input-less.
		return nil, true
	}
	needed := first

	if needed > capacity {
		// Too-small buffer: the host wrote nothing and returned the required length.
		// Re-allocate exactly that and retry once.
		capacity = needed
		ptr, buf = allocBuffer(capacity)
		retry := read(int32(ptr), capacity)
		if retry != needed {
			// The host's answer changed between calls (it should not): bail safely.
			return nil, true
		}
	}

	return buf[:needed], false
}

// emitIntent encodes an OrderIntentBody into the wire bytes and offers it to the
// host.
//
// Field order and encoding match the Rust codec exactly: exchange, asset (String),
// product_type, side, position_side, order_type, time_in_force, quantity (Decimal),
// price (Option<Decimal>), trigger_price (Option<Decimal>), reduce_only (bool). The
// finished bytes are pinned into linear memory so the pointer handed to the host stays
// valid; the emit status is ignored because the order is decided host-side.
func emitIntent(body *OrderIntentBody) {
	// A value-typed Writer backed by the arena: it is a local, so it stays on the stack
	// (no escaping `&Writer{}` heap box), and its buffer lives in linear memory.
	w := newWriter()
	w.PutU8(body.Exchange)
	w.PutString(body.Asset)
	w.PutU8(body.ProductType)
	w.PutU8(body.Side)
	w.PutU8(body.PositionSide)
	w.PutU8(body.OrderType)
	w.PutU8(body.TimeInForce)
	w.PutDecimal(body.Quantity)
	w.PutOptionDecimal(body.Price)
	w.PutOptionDecimal(body.TriggerPrice)
	w.PutBool(body.ReduceOnly)

	bytes := w.Bytes()
	ptr := pin(bytes)
	hostEmitIntent(int32(ptr), int32(len(bytes)))
}

// RunTick runs one tick: read the four inputs (market, ABI v2 window, params, account),
// call the bot, emit any returned intent.
//
// If any read fails (a host error or a truncated message), the tick does nothing
// rather than guessing — there is no partial state to leak, since the host
// re-instantiates the module each tick. The read buffers are not explicitly freed: the
// leaking allocator reclaims them when the instance is torn down at tick end, and the
// decoded asset slice and any parameter lookups alias those buffers until then.
//
// The window is read through the same alloc + single-retry protocol as the snapshot. A
// short or empty window (live warm-up) decodes fine and is handed to the bot as-is; an
// over-cap or malformed window decodes with ok == false and aborts the tick with no
// order, matching how a malformed snapshot fails safe.
func RunTick(bot Bot) {
	// Rewind the bump arena so this tick starts from a clean, deterministic state. The
	// host re-instantiates per tick (which already zeroes the cursor), so this is belt-
	// and-suspenders, but it keeps determinism independent of any instance reuse.
	resetArena()

	// The host import functions are passed by reference directly (they already match the
	// read signature), avoiding a closure that could box onto the Go heap.
	marketBuf, failed := readSnapshot(readMarketData, initialReadCapacity)
	if failed {
		return
	}

	windowBuf, failed := readSnapshot(readMarketWindow, initialReadCapacity)
	if failed {
		return
	}

	paramsBuf, failed := readSnapshot(readStrategyParams, initialReadCapacity)
	if failed {
		return
	}

	accountBuf, failed := readSnapshot(readAccountView, initialReadCapacity)
	if failed {
		return
	}

	market, okMarket := decodeMarketSnapshot(marketBuf)
	window, okWindow := decodeMarketWindow(windowBuf)
	tickParams.buf = paramsBuf
	params := &tickParams
	account, okAccount := decodeAccountView(accountBuf)

	// A malformed market, window, or account message aborts the tick with no emission.
	// The asset slices, candle values, and any parameter lookups still alias the live
	// buffers (or the static candle storage), so the encode below is valid for the rest
	// of the tick.
	if okMarket && okWindow && okAccount {
		body := bot.OnTick(market, window, params, account)
		if body != nil {
			emitIntent(body)
		}
	}
}
