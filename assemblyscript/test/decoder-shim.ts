// Test-only AssemblyScript entry that exposes the PRODUCTION MarketWindow decoder to
// the off-target Node harness, so a test can assert the decoded per-candle VALUES.
//
// This is NOT part of the sample build: it is a separate `asc` entry compiled to its
// own `build/decoder-shim.wasm` by test/decoder.test.mjs. It imports the production
// `MarketWindow.decode` + `candleAt` unchanged and only re-exports thin accessors, so
// it touches no production source and does not bloat the frozen sample module.
//
// The decoded `MarketWindow` aliases the input bytes in linear memory; the harness
// writes the canonical window bytes at a pointer it gets from `alloc`, calls `decode`,
// then reads back the asset and each candle's timestamp + 5 Decimals through the
// accessors below. i64 values cross the boundary to JS as BigInt.

import { MarketWindow, Candle } from "../assembly/types";
import { Decimal } from "../assembly/decimal";

// The window decoded by the most recent `decode` call. A single static slot is enough:
// the harness decodes one window, then reads it, strictly sequentially.
let win: MarketWindow | null = null;

/** Reserves `size` bytes in linear memory for the harness to write window bytes into. */
export function alloc(size: i32): usize {
  return heap.alloc(<usize>size);
}

/**
 * Decodes the canonical window bytes at `(ptr, len)` with the production decoder.
 * Returns the candle count, or `-1` when `MarketWindow.decode` returns `null` (the
 * no-window / fail-safe result: truncated buffer, over-cap count, or short candles).
 */
export function decode(ptr: usize, len: i32): i32 {
  win = MarketWindow.decode(ptr, len);
  return win === null ? -1 : win!.count;
}

/** Absolute pointer to the decoded asset bytes (aliasing the input buffer). */
export function assetPtr(): usize {
  return win!.asset.ptr;
}

/** Byte length of the decoded asset. */
export function assetLen(): i32 {
  return win!.asset.len;
}

/** `timestampMs` of the candle at index `i` (0 oldest), read in place via `candleAt`. */
export function candleTs(i: i32): i64 {
  return win!.candleAt(i).timestampMs;
}

// Selects one of a candle's five OHLCV decimals: 0=open 1=high 2=low 3=close 4=volume.
function pick(c: Candle, field: i32): Decimal {
  if (field == 0) return c.open;
  if (field == 1) return c.high;
  if (field == 2) return c.low;
  if (field == 3) return c.close;
  return c.volume;
}

/** Low 8 bytes of the i128 mantissa of candle `i`'s `field` decimal. */
export function candleDecLow(i: i32, field: i32): i64 {
  return pick(win!.candleAt(i), field).mantissaLow;
}

/** High 8 bytes of the i128 mantissa of candle `i`'s `field` decimal. */
export function candleDecHigh(i: i32, field: i32): i64 {
  return pick(win!.candleAt(i), field).mantissaHigh;
}

/** Scale (fractional digit count) of candle `i`'s `field` decimal. */
export function candleDecScale(i: i32, field: i32): i32 {
  return pick(win!.candleAt(i), field).scale;
}
