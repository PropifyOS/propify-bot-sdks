// The host capability imports the ABI permits — four under v1, five under v2.
//
// Each is declared with `@external("propify", "<name>")` so it lands in the
// `propify` import module under the exact name the host's import allow-list checks.
// Every function is `(ptr: i32, len: i32) -> i32`, matching the host's
// `(i32, i32) -> i32` signature.
//
// `host_read_market_window` is the ABI v2 addition: it serves the bounded
// multi-candle window alongside the single-candle snapshot. A snapshot-only bot
// keeps reading `host_read_market_data`; a window-aware bot also reads the window.
//
// Read functions (`host_read_*`): the guest passes a destination buffer `(ptr,
// len)`. The host writes the encoded snapshot there and returns its full length `n`.
// If `len < n` the host writes NOTHING and returns `n`, letting the guest re-alloc
// and retry. `-1` signals an internal host error.
//
// Emit (`host_emit_intent`): the guest passes `(ptr, len)` of an encoded
// `OrderIntentBody`. The host decodes and bounds-checks it; the returned status is
// not actionable guest-side (the order is decided host-side), so callers ignore it.

@external("propify", "host_read_market_data")
export declare function host_read_market_data(ptr: i32, len: i32): i32;

@external("propify", "host_read_market_window")
export declare function host_read_market_window(ptr: i32, len: i32): i32;

@external("propify", "host_read_strategy_params")
export declare function host_read_strategy_params(ptr: i32, len: i32): i32;

@external("propify", "host_read_account_view")
export declare function host_read_account_view(ptr: i32, len: i32): i32;

@external("propify", "host_emit_intent")
export declare function host_emit_intent(ptr: i32, len: i32): i32;
