// The decoded boundary view types a bot reads, the body it builds, and the six closed
// enums — mirroring the Rust SDK's `propify-sandbox-abi` DTOs and the AssemblyScript
// SDK so the creator's mental model is identical across all three languages.
//
// Enum discriminants are FROZEN and match the Rust codec exactly (see
// `propify-sandbox-abi::wire`). Go's `const` has no way to pin a wire byte to a named
// enum type the way Rust does, so each enum is a group of `uint8` constants, prefixed
// with the enum name (Exchange.Hyperliquid becomes ExchangeHyperliquid). The encoder
// writes the byte directly.
package propify

// Exchange discriminants (FROZEN).
const (
	// ExchangeHyperliquid is the only supported venue in v1.
	ExchangeHyperliquid uint8 = 0
)

// ProductType discriminants (FROZEN).
const (
	ProductTypeSpot uint8 = 0
	ProductTypePerp uint8 = 1
)

// OrderSide discriminants (FROZEN).
const (
	OrderSideBuy  uint8 = 0
	OrderSideSell uint8 = 1
)

// PositionSide discriminants (FROZEN).
const (
	PositionSideLong  uint8 = 0
	PositionSideShort uint8 = 1
)

// OrderType discriminants (FROZEN).
const (
	OrderTypeMarket           uint8 = 0
	OrderTypeLimit            uint8 = 1
	OrderTypeStopMarket       uint8 = 2
	OrderTypeStopLimit        uint8 = 3
	OrderTypeTakeProfitMarket uint8 = 4
	OrderTypeTakeProfitLimit  uint8 = 5
)

// TimeInForce discriminants (FROZEN).
const (
	TimeInForceGtc uint8 = 0
	TimeInForceIoc uint8 = 1
	TimeInForceFok uint8 = 2
	TimeInForceGtx uint8 = 3
)

// AccountStatus discriminants (FROZEN, ABI v3). The account lifecycle status, resolved
// host-side from the challenge attempt and carried in the AccountContext.
const (
	AccountStatusEvaluation uint8 = 0
	AccountStatusFunded     uint8 = 1
)

// DrawdownKind discriminants (FROZEN, ABI v3). Whether the drawdown floor is fixed
// (Static) or rises with the high-water mark (Trailing).
const (
	DrawdownKindStatic   uint8 = 0
	DrawdownKindTrailing uint8 = 1
)

// MarketSnapshot is a single market observation handed to the bot each tick.
//
// Asset is a sub-slice aliasing the host-written input buffer rather than a copied
// string, so a bot can pass the symbol straight through into an order with no
// allocation. TimestampMs is the only clock the guest ever sees.
type MarketSnapshot struct {
	Asset       []byte
	TimestampMs int64
	Open        Decimal
	High        Decimal
	Low         Decimal
	Close       Decimal
	Volume      Decimal
}

// tickMarket, tickParams and tickAccount are package-level decode targets.
//
// Returning the address of a freshly built `&Struct{}` would make it escape and land on
// the Go heap, which traps without `_initialize`. Writing into a static global instead
// and returning its address keeps every decoded view in the data/bss segment — no heap.
// They hold only this tick's data: the host re-instantiates per tick, and resetArena
// runs at tick start, so there is no stale cross-tick state and the single-threaded guest
// never races on them.
var (
	tickMarket  MarketSnapshot
	tickParams  StrategyParams
	tickAccount AccountView
)

// decodeMarketSnapshot decodes a snapshot from the wire bytes. It returns ok == false
// on a truncated buffer, so the driver can skip the tick rather than act on garbage.
func decodeMarketSnapshot(buf []byte) (*MarketSnapshot, bool) {
	r := NewReader(buf)
	asset := r.readString()
	timestampMs := r.readI64()
	open := r.readDecimal()
	high := r.readDecimal()
	low := r.readDecimal()
	close := r.readDecimal()
	volume := r.readDecimal()
	if r.failed {
		return nil, false
	}
	tickMarket = MarketSnapshot{
		Asset:       asset,
		TimestampMs: timestampMs,
		Open:        open,
		High:        high,
		Low:         low,
		Close:       close,
		Volume:      volume,
	}
	return &tickMarket, true
}

// maxCandleCount caps the number of candles in a MarketWindow (ABI v2), matching the
// host's MAX_CANDLE_COUNT (propify-sandbox-abi). A window claiming more than this is
// malformed and decodes with ok == false, exactly as the host rejects an over-cap count.
const maxCandleCount = 256

// Candle is one candle in a MarketWindow: the OHLCV-plus-TimestampMs fields of a
// MarketSnapshot minus the asset (the asset is carried once on the window). Money is the
// exact Decimal, never float64.
type Candle struct {
	TimestampMs int64
	Open        Decimal
	High        Decimal
	Low         Decimal
	Close       Decimal
	Volume      Decimal
}

// MarketWindow is the ABI v2 multi-candle window: the asset plus a bounded, time-ordered
// slice of recent candles, oldest to newest, ending with the latest. A window-aware bot
// recomputes a multi-candle indicator from this host-supplied history each tick rather
// than carrying state across ticks, which keeps it stateless and deterministic. During
// live warm-up, before maxCandleCount candles exist, the window is naturally shorter (and
// may be empty); that is a valid window the bot must tolerate, not a decode failure.
//
// Asset aliases the host-written input buffer (no copy). Candles is a sub-slice of the
// static candleStorage below, so the decode allocates no Go heap.
type MarketWindow struct {
	Asset   []byte
	Candles []Candle
}

// tickWindow and candleStorage are the static decode targets for the window, mirroring
// tickMarket. Returning the address of a freshly built window, or building the candle
// slice with make/append, would escape onto the Go heap, which traps without
// `_initialize`. A package-level struct plus a fixed candle array keep every decoded
// candle in the data/bss segment — no heap. They hold only this tick's data: the host
// re-instantiates per tick, resetArena runs at tick start, and the single-threaded guest
// never races on them.
var (
	tickWindow    MarketWindow
	candleStorage [maxCandleCount]Candle
)

// decodeMarketWindow decodes a window from v2 wire bytes: the asset string, a u32 candle
// count, then that many fixed-width candles. It returns ok == false — so the driver skips
// the tick rather than act on garbage — on a truncated buffer or an over-cap count. An
// empty (count == 0) window decodes successfully, which is the live warm-up path. Candles
// are written into the static candleStorage and the returned window aliases it, so the
// decode never touches the Go heap.
func decodeMarketWindow(buf []byte) (*MarketWindow, bool) {
	r := NewReader(buf)
	asset := r.readString()
	count := r.readU32()
	if r.failed {
		return nil, false
	}
	// Reject an over-cap count exactly as the host does, before it can index the array.
	if count > maxCandleCount {
		return nil, false
	}
	for i := uint32(0); i < count; i++ {
		timestampMs := r.readI64()
		open := r.readDecimal()
		high := r.readDecimal()
		low := r.readDecimal()
		closePrice := r.readDecimal()
		volume := r.readDecimal()
		if r.failed {
			return nil, false
		}
		candleStorage[i] = Candle{
			TimestampMs: timestampMs,
			Open:        open,
			High:        high,
			Low:         low,
			Close:       closePrice,
			Volume:      volume,
		}
	}
	tickWindow = MarketWindow{Asset: asset, Candles: candleStorage[:count]}
	return &tickWindow, true
}

// StrategyParams is the read-only strategy parameters for this tick: a count-prefixed
// list of (name, value) pairs.
//
// Decoding is lazy and copy-free: the view holds the raw buffer and scans it on demand
// in Find, mirroring the Rust SDK's `params.iter().find(...)`. Scanning avoids
// materializing a slice of strings and keeps lookups independent of parameter order.
type StrategyParams struct {
	buf []byte
}

// Find returns the Decimal value of the first parameter whose name equals the given
// bytes and found == true, or the zero Decimal and found == false when no parameter
// has that name. Names are compared byte-for-byte with no UTF-8 decoding.
//
// The value is returned BY VALUE, not as a pointer: Decimal is a small value type (two
// 64-bit mantissa halves and a scale), so the return copies those integer fields with
// no make/append, no heap allocation, and no GC dependency — safe on the wasm-unknown
// target that never calls `_initialize`. Returning by value also makes the API safe by
// construction: a caller may look up several parameters and hold each result, and a
// later Find never disturbs an earlier one. (The previous pointer API aliased a single
// shared static slot, so a second lookup silently overwrote the first.)
func (p *StrategyParams) Find(name []byte) (Decimal, bool) {
	r := NewReader(p.buf)
	count := r.readU32()
	if r.failed {
		return Decimal{}, false
	}
	for i := uint32(0); i < count; i++ {
		nameSlice := r.readString()
		value := r.readDecimal()
		if r.failed {
			return Decimal{}, false
		}
		if bytesEqual(nameSlice, name) {
			return value, true
		}
	}
	return Decimal{}, false
}

// AccountView is this account's own figures — the only account data the guest may
// read. There is deliberately no account id and no peer data on the wire.
type AccountView struct {
	Equity          Decimal
	AvailableMargin Decimal
	Exposure        Decimal
	UnrealizedPnl   Decimal
}

// decodeAccountView decodes an account view from the wire bytes, returning ok == false
// on a truncated buffer.
func decodeAccountView(buf []byte) (*AccountView, bool) {
	r := NewReader(buf)
	equity := r.readDecimal()
	availableMargin := r.readDecimal()
	exposure := r.readDecimal()
	unrealizedPnl := r.readDecimal()
	if r.failed {
		return nil, false
	}
	tickAccount = AccountView{
		Equity:          equity,
		AvailableMargin: availableMargin,
		Exposure:        exposure,
		UnrealizedPnl:   unrealizedPnl,
	}
	return &tickAccount, true
}

// maxLeverageOverrideCount and maxAllowedInstrumentCount cap the two count-prefixed lists
// in an AccountContext, matching the host's MAX_LEVERAGE_OVERRIDE_COUNT (64) and
// MAX_ALLOWED_INSTRUMENT_COUNT (1024) in propify-sandbox-abi. A list claiming more than
// its cap is malformed and decodes with ok == false, exactly as the host rejects an
// over-cap count before it can index the static storage below.
const (
	maxLeverageOverrideCount  = 64
	maxAllowedInstrumentCount = 1024
)

// DrawdownRule is the resolved drawdown rule inside an AccountContext (ABI v3): the kind
// discriminant plus three host-computed decimals. Floor is the authoritative current
// line the bot should act on; for a Trailing account it already reflects the high-water
// mark, for a Static account it is the fixed anchor minus the limit. Kind is carried as a
// raw uint8 discriminant (DrawdownKindStatic/DrawdownKindTrailing), mirroring how the
// other enums ride as raw bytes on this boundary.
type DrawdownRule struct {
	Kind          uint8
	Limit         Decimal
	Floor         Decimal
	HighWaterMark Decimal
}

// LeverageOverride is one per-asset-class leverage cap: the asset-class name (a sub-slice
// aliasing the host-written input buffer, no copy) and the cap. It mirrors the Rust
// (String, Decimal) override pair.
type LeverageOverride struct {
	AssetClass []byte
	Cap        Decimal
}

// AccountContext is the read-only account context handed to a v3 bot each tick: the
// lifecycle status plus the resolved rule set (daily-loss floor, drawdown kind and floor,
// leverage caps, allowed instruments). The in-bot rules are for ADAPTATION, not trust —
// host-side risk remains the sole backstop — so a well-behaved bot can size down near a
// floor or respect a leverage cap, but cannot exceed a limit by ignoring it.
//
// Status is a raw AccountStatus discriminant. LeverageOverrides aliases the static
// leverageOverrideStorage and AllowedInstruments aliases allowedInstrumentStorage, and
// each instrument/asset-class slice aliases the host-written input buffer, so the decode
// allocates no Go heap.
type AccountContext struct {
	Status             uint8
	DailyLossLimit     Decimal
	DailyLossFloor     Decimal
	Drawdown           DrawdownRule
	DefaultLeverage    Decimal
	LeverageOverrides  []LeverageOverride
	AllowedInstruments [][]byte
}

// tickContext, leverageOverrideStorage and allowedInstrumentStorage are the static decode
// targets for the account context, mirroring tickWindow/candleStorage. Returning the
// address of a freshly built context, or building the lists with make/append, would escape
// onto the Go heap, which traps without `_initialize`. A package-level struct plus
// fixed-size arrays keep every decoded element in the data/bss segment — no heap. They
// hold only this tick's data: the host re-instantiates per tick, resetArena runs at tick
// start, and the single-threaded guest never races on them.
var (
	tickContext              AccountContext
	leverageOverrideStorage  [maxLeverageOverrideCount]LeverageOverride
	allowedInstrumentStorage [maxAllowedInstrumentCount][]byte
)

// decodeAccountContext decodes an account context from v3 wire bytes, mirroring the host
// codec's byte layout EXACTLY: status (u8), daily_loss_limit and daily_loss_floor
// (Decimal), the inline DrawdownRule (kind u8 + 3 Decimals), default_leverage (Decimal), a
// u32-count-prefixed list of (asset_class String, cap Decimal) overrides, and a
// u32-count-prefixed list of allowed-instrument Strings. It returns ok == false — so the
// driver skips the tick rather than act on garbage — on a truncated buffer or an over-cap
// count. Enum discriminants ride as raw bytes (the host is the trusted producer and always
// sends valid values), consistent with how Decimal ranges are likewise carried unchecked.
// The lists are written into the static storage and the returned context aliases it, so
// the decode never touches the Go heap.
func decodeAccountContext(buf []byte) (*AccountContext, bool) {
	r := NewReader(buf)
	status := r.readU8()
	dailyLossLimit := r.readDecimal()
	dailyLossFloor := r.readDecimal()
	drawdownKind := r.readU8()
	drawdownLimit := r.readDecimal()
	drawdownFloor := r.readDecimal()
	drawdownHWM := r.readDecimal()
	defaultLeverage := r.readDecimal()
	if r.failed {
		return nil, false
	}

	overrideCount := r.readU32()
	if r.failed {
		return nil, false
	}
	// Reject an over-cap count exactly as the host does, before it can index the array.
	if overrideCount > maxLeverageOverrideCount {
		return nil, false
	}
	for i := uint32(0); i < overrideCount; i++ {
		assetClass := r.readString()
		leverageCap := r.readDecimal()
		if r.failed {
			return nil, false
		}
		leverageOverrideStorage[i] = LeverageOverride{AssetClass: assetClass, Cap: leverageCap}
	}

	instrumentCount := r.readU32()
	if r.failed {
		return nil, false
	}
	if instrumentCount > maxAllowedInstrumentCount {
		return nil, false
	}
	for i := uint32(0); i < instrumentCount; i++ {
		instrument := r.readString()
		if r.failed {
			return nil, false
		}
		allowedInstrumentStorage[i] = instrument
	}

	tickContext = AccountContext{
		Status:             status,
		DailyLossLimit:     dailyLossLimit,
		DailyLossFloor:     dailyLossFloor,
		Drawdown:           DrawdownRule{Kind: drawdownKind, Limit: drawdownLimit, Floor: drawdownFloor, HighWaterMark: drawdownHWM},
		DefaultLeverage:    defaultLeverage,
		LeverageOverrides:  leverageOverrideStorage[:overrideCount],
		AllowedInstruments: allowedInstrumentStorage[:instrumentCount],
	}
	return &tickContext, true
}

// OrderIntentBody is the intent a bot emits: an order minus the two fields the guest
// may not set (intent_id, which needs a clock the guest is denied, and the account,
// which the guest must not name). The host stamps those when it lifts the body into a
// full OrderIntent.
//
// Asset is a byte slice so the bot can pass the snapshot's symbol straight through.
// Price and TriggerPrice are *Decimal, mirroring the Rust Option<Decimal>: nil means
// None.
type OrderIntentBody struct {
	Exchange     uint8
	Asset        []byte
	ProductType  uint8
	Side         uint8
	PositionSide uint8
	OrderType    uint8
	TimeInForce  uint8
	Quantity     Decimal
	Price        *Decimal
	TriggerPrice *Decimal
	ReduceOnly   bool
}
