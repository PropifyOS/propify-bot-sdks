// The creator-facing `Bot` abstraction and the tick driver, mirroring the Rust
// SDK's `trait Bot` + `run_tick`.
//
// A creator subclasses `Bot` and implements `onTick`; the sample's entry module
// wires the four ABI exports to the helpers here (AS has no macro system, so the
// wiring is a few one-line `export function`s instead of Rust's `register_bot!`).
// The driver performs the read -> onTick -> emit plumbing against the host imports
// (four under v1, five under v2 with the window read), exactly like the Rust
// `run_tick` does against `HostBindings`.

import {
  host_read_market_data,
  host_read_market_window,
  host_read_strategy_params,
  host_read_account_view,
  host_emit_intent,
} from "./imports";
import { Decimal } from "./decimal";
import { Writer } from "./wire";
import {
  AccountView,
  MarketSnapshot,
  MarketWindow,
  OrderIntentBody,
  StrategyParams,
} from "./types";

/**
 * The ABI major version this SDK targets, returned by the `abi_version` export.
 * Matches `propify-sandbox-abi::ABI_VERSION` (2 for the multi-candle window); the host
 * dual-supports v1 and v2 guests and refuses any other value before a tick runs.
 */
export const ABI_VERSION: i32 = 2;

/**
 * Initial size, in bytes, of the buffer the SDK allocates before a host read. A
 * market snapshot, a small parameter list, and an account view all encode well under
 * this, so the common case is a single host call with no retry.
 */
const INITIAL_READ_CAPACITY: i32 = 256;

/**
 * A trading bot a creator implements. On each tick the driver hands it the decoded
 * inputs and asks for at most one order.
 *
 * The implementation must be a pure, deterministic function of its inputs: no clock,
 * no randomness, no `f64`. Return an [`OrderIntentBody`] to emit one order, or `null`
 * to do nothing (mirroring the Rust `Option<OrderIntentBody>`). Returning a body does
 * not guarantee placement — the host still bounds-checks and risk-gates it.
 *
 * `window` (ABI v2) is the host-supplied candle history: the asset plus a time-ordered
 * array of recent candles, oldest to newest, ending with the latest. A snapshot-only
 * bot ignores it and reads only `market`; a window-aware bot recomputes a multi-candle
 * indicator from it each tick. During live warm-up the window is shorter (and may be
 * empty); the SDK hands it through as-is and the bot decides how to handle a short
 * window — the SDK does not special-case warm-up.
 */
export abstract class Bot {
  abstract onTick(
    market: MarketSnapshot,
    window: MarketWindow,
    params: StrategyParams,
    account: AccountView
  ): OrderIntentBody | null;
}

/** A read function imported from the host: `(ptr, len) -> status/length`. */
type HostRead = (ptr: i32, len: i32) => i32;

/**
 * The outcome of one host read: the live buffer `(ptr, len)` on success, or a
 * `failed` flag. The buffer is kept alive for the whole tick so decoded byte slices
 * may alias it; the driver frees it at the end.
 */
class ReadBuffer {
  constructor(public ptr: usize, public len: i32, public failed: bool) {}
}

/**
 * Reads one host snapshot using the documented alloc + single-retry protocol.
 *
 * Allocate a guess buffer and call the host. The host returns the snapshot's full
 * length `n`. If the buffer was too small the host wrote nothing and `n > capacity`,
 * so the SDK frees the buffer, allocates exactly `n`, and retries once. A negative
 * return is the host's internal error, surfaced as `failed` ("no input this tick").
 * The too-small buffer is freed here; the returned live buffer is freed by the
 * caller after the tick.
 */
function readSnapshot(read: HostRead, initialCapacity: i32): ReadBuffer {
  let capacity = initialCapacity > 0 ? initialCapacity : 1;
  let ptr = heap.alloc(<usize>capacity);

  const first = read(<i32>ptr, capacity);
  if (first < 0) {
    // Internal host error: free the buffer and treat the tick as input-less.
    heap.free(ptr);
    return new ReadBuffer(0, 0, true);
  }
  const needed = first;

  if (needed > capacity) {
    // Too-small buffer: the host wrote nothing and returned the required length.
    // Re-allocate exactly that and retry once.
    heap.free(ptr);
    capacity = needed;
    ptr = heap.alloc(<usize>capacity);
    const retry = read(<i32>ptr, capacity);
    if (retry != needed) {
      // The host's answer changed between calls (it should not): bail safely.
      heap.free(ptr);
      return new ReadBuffer(0, 0, true);
    }
  }

  return new ReadBuffer(ptr, needed, false);
}

/**
 * Encodes an [`OrderIntentBody`] into the wire bytes and offers it to the host.
 *
 * Field order and encoding match the Rust codec exactly: exchange, asset (String),
 * product_type, side, position_side, order_type, time_in_force, quantity (Decimal),
 * price (Option<Decimal>), trigger_price (Option<Decimal>), reduce_only (bool). The
 * emit status is ignored (the order is decided host-side); the buffer is freed after.
 */
function emitIntent(body: OrderIntentBody): void {
  const writer = new Writer(64);
  writer.putU8(body.exchange);
  writer.putString(body.asset.ptr, body.asset.len);
  writer.putU8(body.productType);
  writer.putU8(body.side);
  writer.putU8(body.positionSide);
  writer.putU8(body.orderType);
  writer.putU8(body.timeInForce);
  writer.putDecimal(body.quantity);
  writer.putOptionDecimal(body.price);
  writer.putOptionDecimal(body.triggerPrice);
  writer.putBool(body.reduceOnly);

  host_emit_intent(<i32>writer.pointer, writer.length);
  writer.free();
}

/**
 * Runs one tick: read the four inputs (market, ABI v2 window, params, account), call
 * the bot, emit any returned intent.
 *
 * If any read fails (a host error or a truncated message), the tick does nothing
 * rather than guessing — there is no partial state to leak, since the host
 * re-instantiates the module each tick. Every read buffer is freed before returning
 * (pairing each alloc with one dealloc), after the encode that may alias it.
 *
 * The window is read through the same alloc + single-retry protocol as the snapshot. A
 * short or empty window (live warm-up) decodes fine and is handed to the bot as-is; an
 * over-cap or malformed window decodes to `null` and aborts the tick with no order,
 * matching how a malformed snapshot fails safe.
 */
export function runTick(bot: Bot): void {
  const marketBuf = readSnapshot(host_read_market_data, INITIAL_READ_CAPACITY);
  if (marketBuf.failed) return;

  const windowBuf = readSnapshot(
    host_read_market_window,
    INITIAL_READ_CAPACITY
  );
  if (windowBuf.failed) {
    heap.free(marketBuf.ptr);
    return;
  }

  const paramsBuf = readSnapshot(
    host_read_strategy_params,
    INITIAL_READ_CAPACITY
  );
  if (paramsBuf.failed) {
    heap.free(marketBuf.ptr);
    heap.free(windowBuf.ptr);
    return;
  }

  const accountBuf = readSnapshot(host_read_account_view, INITIAL_READ_CAPACITY);
  if (accountBuf.failed) {
    heap.free(marketBuf.ptr);
    heap.free(windowBuf.ptr);
    heap.free(paramsBuf.ptr);
    return;
  }

  const market = MarketSnapshot.decode(marketBuf.ptr, marketBuf.len);
  const window = MarketWindow.decode(windowBuf.ptr, windowBuf.len);
  const params = new StrategyParams(paramsBuf.ptr, paramsBuf.len);
  const account = AccountView.decode(accountBuf.ptr, accountBuf.len);

  // A malformed market, window, or account message aborts the tick with no emission.
  // The asset slices, candle bytes, and any param lookups still alias the live buffers,
  // so the encode below (inside onTick's returned body) is valid until the frees.
  if (market !== null && window !== null && account !== null) {
    const body = bot.onTick(market, window, params, account);
    if (body !== null) emitIntent(body);
  }

  heap.free(marketBuf.ptr);
  heap.free(windowBuf.ptr);
  heap.free(paramsBuf.ptr);
  heap.free(accountBuf.ptr);
}

// --- ABI export helpers ----------------------------------------------------
//
// The sample's entry module delegates its `abi_version`/`alloc`/`dealloc` exports to
// these one-liners so the wiring stays trivial. `on_tick` delegates to `runTick`.

/** Backs the `abi_version` export: returns the ABI v2 version this SDK targets. */
export function abiVersion(): i32 {
  return ABI_VERSION;
}

/**
 * Backs the `alloc` export: reserve `size` bytes in linear memory, return the
 * offset (or 0 on a non-positive size). The host writes snapshot bytes into buffers
 * the guest reserves this way.
 */
export function alloc(size: i32): i32 {
  if (size <= 0) return 0;
  return <i32>heap.alloc(<usize>size);
}

/**
 * Backs the `dealloc` export: release a buffer previously returned by `alloc`. Under
 * the `stub` runtime this is a no-op (the bump allocator never frees), which is fine
 * because the host re-instantiates the module fresh every tick, resetting memory.
 */
export function dealloc(ptr: i32, size: i32): void {
  if (ptr == 0) return;
  heap.free(<usize>ptr);
}
