//go:build tinygo

// The host capability imports the ABI permits — four under v1, five under v2.
//
// Each is declared bodyless with a `//go:wasmimport propify <name>` directive, so it
// lands in the `propify` import module under the exact name the host's import
// allow-list checks. Every function is `(ptr int32, length int32) -> int32`, matching
// the host's `(i32, i32) -> i32` signature. Only int32 crosses the boundary, as the
// ABI requires.
//
// host_read_market_window is the ABI v2 addition: it serves the bounded multi-candle
// window alongside the single-candle snapshot. A snapshot-only bot keeps reading
// host_read_market_data; a window-aware bot also reads the window.
//
// The blank `unsafe` import is the toolchain's required marker for a file that uses
// `//go:wasmimport`; it pulls in no code.
//
// Read functions (host_read_*): the guest passes a destination buffer (ptr, length).
// The host writes the encoded snapshot there and returns its full length n. If
// length < n the host writes NOTHING and returns n, letting the guest re-alloc and
// retry. A negative return signals an internal host error.
//
// Emit (host_emit_intent): the guest passes (ptr, length) of an encoded
// OrderIntentBody. The host decodes and bounds-checks it; the returned status is not
// actionable guest-side (the order is decided host-side), so callers ignore it.
package propify

import _ "unsafe" // required for the //go:wasmimport directives below

//go:wasmimport propify host_read_market_data
func hostReadMarketData(ptr int32, length int32) int32

//go:wasmimport propify host_read_market_window
func hostReadMarketWindow(ptr int32, length int32) int32

//go:wasmimport propify host_read_strategy_params
func hostReadStrategyParams(ptr int32, length int32) int32

//go:wasmimport propify host_read_account_view
func hostReadAccountView(ptr int32, length int32) int32

//go:wasmimport propify host_emit_intent
func hostEmitIntent(ptr int32, length int32) int32
