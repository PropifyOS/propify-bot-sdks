// Test-only AssemblyScript entry that exposes the PRODUCTION AccountContext decoder to
// the off-target Node harness, so a test can assert the decoded per-field VALUES.
//
// This is NOT part of the sample build: it is a separate `asc` entry compiled to its own
// `build/account-context-shim.wasm` by test/account-context.test.mjs. It imports the
// production `AccountContext.decode` unchanged and only re-exports thin accessors, so it
// touches no production source and does not bloat the frozen sample module. It mirrors the
// decoder-shim.ts approach used for MarketWindow.
//
// The decoded `AccountContext` aliases the input bytes in linear memory; the harness writes
// the canonical context bytes at a pointer it gets from `alloc`, calls `decode`, then reads
// back the scalar fields, the drawdown sub-shape, and both count-prefixed lists through the
// accessors below. i64 values cross the boundary to JS as BigInt.

import { AccountContext } from "../assembly/types";
import { Decimal } from "../assembly/decimal";

// The context decoded by the most recent `decode` call. A single static slot is enough:
// the harness decodes one context, then reads it, strictly sequentially.
let ctx: AccountContext | null = null;

/** Reserves `size` bytes in linear memory for the harness to write context bytes into. */
export function alloc(size: i32): usize {
  return heap.alloc(<usize>size);
}

/**
 * Decodes the canonical context bytes at `(ptr, len)` with the production decoder.
 * Returns `1` on success, or `-1` when `AccountContext.decode` returns `null` (the
 * fail-safe result: truncated buffer or an over-cap list count).
 */
export function decode(ptr: usize, len: i32): i32 {
  ctx = AccountContext.decode(ptr, len);
  return ctx === null ? -1 : 1;
}

/** The decoded `status` discriminant (AccountStatus). */
export function status(): i32 {
  return <i32>ctx!.status;
}

/** The decoded `drawdown.kind` discriminant (DrawdownKind). */
export function drawdownKind(): i32 {
  return <i32>ctx!.drawdown.kind;
}

// Selects one of the context's top-level decimals by index, so the three (low/high/scale)
// accessors stay compact: 0=dailyLossLimit 1=dailyLossFloor 2=drawdown.limit
// 3=drawdown.floor 4=drawdown.highWaterMark 5=defaultLeverage.
function pickDecimal(field: i32): Decimal {
  if (field == 0) return ctx!.dailyLossLimit;
  if (field == 1) return ctx!.dailyLossFloor;
  if (field == 2) return ctx!.drawdown.limit;
  if (field == 3) return ctx!.drawdown.floor;
  if (field == 4) return ctx!.drawdown.highWaterMark;
  return ctx!.defaultLeverage;
}

/** Low 8 bytes of the i128 mantissa of the `field`-th top-level decimal. */
export function decLow(field: i32): i64 {
  return pickDecimal(field).mantissaLow;
}

/** High 8 bytes of the i128 mantissa of the `field`-th top-level decimal. */
export function decHigh(field: i32): i64 {
  return pickDecimal(field).mantissaHigh;
}

/** Scale (fractional digit count) of the `field`-th top-level decimal. */
export function decScale(field: i32): i32 {
  return pickDecimal(field).scale;
}

/** Number of leverage overrides in the decoded context. */
export function overrideCount(): i32 {
  return ctx!.leverageOverrides.length;
}

/** Absolute pointer to override `i`'s asset-class bytes (aliasing the input buffer). */
export function overrideAssetPtr(i: i32): usize {
  return ctx!.leverageOverrides[i].assetClass.ptr;
}

/** Byte length of override `i`'s asset-class name. */
export function overrideAssetLen(i: i32): i32 {
  return ctx!.leverageOverrides[i].assetClass.len;
}

/** Low 8 bytes of the i128 mantissa of override `i`'s cap. */
export function overrideCapLow(i: i32): i64 {
  return ctx!.leverageOverrides[i].cap.mantissaLow;
}

/** High 8 bytes of the i128 mantissa of override `i`'s cap. */
export function overrideCapHigh(i: i32): i64 {
  return ctx!.leverageOverrides[i].cap.mantissaHigh;
}

/** Scale of override `i`'s cap. */
export function overrideCapScale(i: i32): i32 {
  return ctx!.leverageOverrides[i].cap.scale;
}

/** Number of allowed instruments in the decoded context. */
export function instrumentCount(): i32 {
  return ctx!.allowedInstruments.length;
}

/** Absolute pointer to instrument `i`'s bytes (aliasing the input buffer). */
export function instrumentPtr(i: i32): usize {
  return ctx!.allowedInstruments[i].ptr;
}

/** Byte length of instrument `i`. */
export function instrumentLen(i: i32): i32 {
  return ctx!.allowedInstruments[i].len;
}

/** `1` when the decoded context carries a profit target (`Some`), `0` when `null`. */
export function profitTargetPresent(): i32 {
  return ctx!.profitTarget === null ? 0 : 1;
}

/** Low 8 bytes of the i128 mantissa of the profit target (caller checks present first). */
export function profitTargetLow(): i64 {
  return ctx!.profitTarget!.mantissaLow;
}

/** High 8 bytes of the i128 mantissa of the profit target. */
export function profitTargetHigh(): i64 {
  return ctx!.profitTarget!.mantissaHigh;
}

/** Scale of the profit target. */
export function profitTargetScale(): i32 {
  return ctx!.profitTarget!.scale;
}
