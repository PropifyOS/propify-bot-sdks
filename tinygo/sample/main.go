// A trivial, fully deterministic sample bot — the TinyGo counterpart of the Rust
// `propify-bot-sdk-sample` and the AssemblyScript `sample.ts`.
//
// It reads the market snapshot and the strategy parameters, then emits one fixed market
// BUY for the snapshot's asset. It exists to exercise the SDK and to be the fixture the
// host's acceptance test loads (compiled to build/sample.wasm). It is NOT a product
// strategy.
//
// The rule is a pure function of the inputs — no clock, no randomness, no float64: buy
// `quantity` units of `market.Asset`, where `quantity` is the "quantity" strategy
// parameter if present, else a fixed default of 0.001. Identical inputs always produce
// identical emitted bytes.
//
// This is the compiled entry (package main). It implements the bot and exports the four
// ABI functions by delegating to the SDK helpers — the Go equivalent of Rust's
// `register_bot!` macro, written by hand because Go has no macros. The linear `memory`
// export is emitted automatically by the wasm-unknown target. An empty `main` is
// required because this is `package main`; the wasm-unknown target does not call it and
// does not export a `_start`.
package main

import "github.com/PropifyOS/propify-bot-sdks/tinygo/propify"

// quantityName is the ASCII bytes of the parameter name "quantity", held as a package
// value so the lookup is a plain byte comparison with no UTF-8 decoding.
// q,u,a,n,t,i,t,y = 113,117,97,110,116,105,116,121.
var quantityName = []byte{113, 117, 97, 110, 116, 105, 116, 121}

// SampleBot is the sample strategy. Stateless: the host re-instantiates the guest each
// tick, so the decision depends only on this tick's inputs. It is a zero-size struct, so
// boxing it into the SDK's `Bot` interface needs no heap allocation.
type SampleBot struct{}

// orderBody is the static slot the sample emits from. Returning the address of a fresh
// `&propify.OrderIntentBody{}` would escape through the interface call onto the Go heap,
// which traps on the `wasm-unknown` target because the host never calls `_initialize` to
// bootstrap the allocator. Filling a package-level value and returning its address keeps
// the body in linear memory's data/bss segment. The guest is single-threaded and the
// host re-instantiates per tick, so the static slot holds only this tick's order.
var orderBody propify.OrderIntentBody

// OnTick takes the order size from the "quantity" parameter when present, else the
// fixed default 0.001, and returns a single deterministic market BUY for the snapshot's
// asset. Looking the size up by name keeps the bot independent of parameter ordering.
func (SampleBot) OnTick(
	market *propify.MarketSnapshot,
	window *propify.MarketWindow,
	params *propify.StrategyParams,
	account *propify.AccountView,
) *propify.OrderIntentBody {
	// The sample is a snapshot-only bot: it ignores the ABI v2 window and decides from
	// the latest candle alone.
	_ = window
	// Find returns the size by value plus a found flag, so the lookup needs no pointer
	// and no heap. When the parameter is absent, fall back to the fixed default.
	quantity, ok := params.Find(quantityName)
	if !ok {
		// Default 0.001, built from an exact (mantissa, scale) = (1, 3) so no float64
		// is involved.
		quantity = propify.DecimalFromI64(1, 3)
	}

	orderBody = propify.OrderIntentBody{
		Exchange:     propify.ExchangeHyperliquid,
		Asset:        market.Asset, // pass the snapshot's symbol straight through, no copy
		ProductType:  propify.ProductTypePerp,
		Side:         propify.OrderSideBuy,
		PositionSide: propify.PositionSideLong,
		OrderType:    propify.OrderTypeMarket,
		TimeInForce:  propify.TimeInForceIoc,
		Quantity:     quantity,
		Price:        nil, // market order carries no limit price
		TriggerPrice: nil, // no trigger price
		ReduceOnly:   false,
	}
	return &orderBody
}

// --- ABI exports (the hand-written equivalent of register_bot!) ------------
//
// Each directive exports the function under the exact name the ABI requires,
// regardless of the Go function name. `memory` is exported automatically by the target.
//
// These deliberately use the legacy `//export name` directive, NOT the newer
// `//go:wasmexport name`. The newer directive implements the Go reactor lifecycle: the
// compiler wraps each export in a `runtime.wasmExportCheckRun` guard that traps with
// `unreachable` unless `_initialize` (or `_start`) has run first to set the runtime's
// `initializeCalled` flag. The host calls neither — its contract is `abi_version`
// then `on_tick`, with `alloc`/`dealloc` as signature-checked exports — so every
// `//go:wasmexport` entry, even `abi_version`'s `return 2`, would trap before any of our
// code runs. `//export` emits a plain wasm export with no such guard, which is exactly what
// a host that does not drive the reactor lifecycle needs. This is sound only because the
// guest never touches the Go heap or any state that `_initialize` would set up (see the
// static-arena allocator in propify/memory.go and the static decode targets): every export
// runs correctly from a freshly instantiated, data-segment-zeroed module with no init call.

//export abi_version
func abiVersion() int32 {
	return propify.AbiVersion()
}

//export alloc
func alloc(size int32) int32 {
	return propify.Alloc(size)
}

//export dealloc
func dealloc(ptr int32, size int32) {
	propify.Dealloc(ptr, size)
}

// sampleBot is a package-level static instance the tick driver runs. Passing its address
// (a pointer to a global) into the SDK's `Bot` interface stores a pointer in the interface
// value, which needs no heap allocation. Boxing the `SampleBot{}` value directly instead
// would call `runtime.alloc`, and any heap allocation traps on this target because the host
// never calls `_initialize`. *SampleBot satisfies Bot since OnTick has a value receiver.
var sampleBot SampleBot

//export on_tick
func onTick() {
	// Fresh per tick: the host re-instantiates the module each tick, so there is no
	// state to carry over.
	propify.RunTick(&sampleBot)
}

// main is required by package main but is never invoked by the wasm-unknown target.
func main() {}
