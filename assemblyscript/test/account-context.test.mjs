// Off-target unit tests for the AssemblyScript AccountContext DECODER (ABI v3).
//
// The sample.test.mjs serves an account context to the snapshot-only sample, but the
// sample ignores it, so no EXECUTED assertion there covers the decoded per-field VALUES.
// These tests close that gap: they compile a tiny test-only shim
// (test/account-context-shim.ts) that re-exports the PRODUCTION AccountContext.decode
// unchanged, feed it canonical context bytes, and assert the status, both daily-loss
// decimals, the inline drawdown sub-shape (kind + 3 decimals), the default leverage, and
// every element of the two count-prefixed lists (leverage overrides and allowed
// instruments) round-trip to the exact values encoded. Fail-safe cases (over-cap counts,
// truncated buffers) must yield the no-context result.
//
// The shim is NOT the sample: it is a separate asc entry built to
// build/account-context-shim.wasm (gitignored), so no production source is touched and the
// frozen sample module is not bloated. The fixture encoders below are hand-written and
// independent of the SDK codec, so a shared bug cannot hide a decoder bug.
//
// Zero test dependencies beyond Node's built-ins. The shim is compiled in a before() hook
// via the pinned asc, so `pnpm test` is self-contained.

import { test, before } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const PKG_ROOT = fileURLToPath(new URL("..", import.meta.url));
const ASC_BIN = fileURLToPath(new URL("../node_modules/.bin/asc", import.meta.url));
const SHIM_WASM = fileURLToPath(new URL("../build/account-context-shim.wasm", import.meta.url));

// Caps mirrored from propify-sandbox-abi (and the AS MAX_* constants).
const MAX_LEVERAGE_OVERRIDE_COUNT = 64;
const MAX_ALLOWED_INSTRUMENT_COUNT = 1024;

// --- canonical v3 wire encoders (little-endian), independent of the SDK -------

const u32le = (n) => [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
const i32le = (n) => u32le(n >>> 0);

const i64le = (big) => {
  let v = BigInt.asUintN(64, BigInt(big));
  const out = [];
  for (let i = 0; i < 8; i++) {
    out.push(Number(v & 0xffn));
    v >>= 8n;
  }
  return out;
};

const strBytes = (s) => {
  const enc = new TextEncoder().encode(s);
  return [...u32le(enc.length), ...enc];
};

// A Decimal on the wire: i128 mantissa (16 LE = low half then high half) + i32 scale.
// lo/hi are the SIGNED 64-bit mantissa halves (as the i64 the decoder returns).
const decBytes = (d) => [...i64le(d.lo), ...i64le(d.hi), ...i32le(d.scale)];

// An AccountContext on the wire, mirroring the host codec layout EXACTLY: status (u8),
// daily_loss_limit + daily_loss_floor (Decimal), the inline DrawdownRule (kind u8 +
// limit/floor/high_water_mark Decimals), default_leverage (Decimal), a u32-count-prefixed
// list of (asset_class String, cap Decimal) overrides, then a u32-count-prefixed list of
// allowed-instrument Strings, then an Option<Decimal> profit_target tail (tag 0 = None,
// tag 1 = Some + the decimal).
const contextBytes = (c) => {
  const out = [
    c.status,
    ...decBytes(c.dailyLossLimit),
    ...decBytes(c.dailyLossFloor),
    c.drawdown.kind,
    ...decBytes(c.drawdown.limit),
    ...decBytes(c.drawdown.floor),
    ...decBytes(c.drawdown.hwm),
    ...decBytes(c.defaultLeverage),
    ...u32le(c.overrides.length),
  ];
  for (const o of c.overrides) out.push(...strBytes(o.assetClass), ...decBytes(o.cap));
  out.push(...u32le(c.instruments.length));
  for (const s of c.instruments) out.push(...strBytes(s));
  if (c.profitTarget == null) {
    out.push(0);
  } else {
    out.push(1, ...decBytes(c.profitTarget));
  }
  return out;
};

// --- fixtures: distinct, non-trivial decimals across every field --------------
// A small positive, a tiny fraction, a NEGATIVE mantissa (low -N / high all-ones -1), a
// LARGE i128 whose high half is non-zero, max scale 28, and exact zero. No two collide.

const d = (lo, hi, scale) => ({ lo: BigInt(lo), hi: BigInt(hi), scale });

// Top-level decimal fixtures, indexed to match the shim's pickDecimal selector:
// 0=dailyLossLimit 1=dailyLossFloor 2=drawdown.limit 3=drawdown.floor
// 4=drawdown.highWaterMark 5=defaultLeverage.
const DECIMALS = [
  d(200000, 0, 2), // dailyLossLimit   2000.00
  d(-12345, -1, 2), // dailyLossFloor  -123.45 (negative i128)
  d(2, 0, 3), // drawdown.limit         0.002 (tiny fraction)
  d(7, 1, 0), // drawdown.floor         1<<64 + 7 (high half set)
  d(1, 0, 28), // drawdown.hwm          1e-28 (max scale)
  d(0, 0, 0), // defaultLeverage        exactly zero
];

const FULL_CONTEXT = {
  status: 1, // Funded
  dailyLossLimit: DECIMALS[0],
  dailyLossFloor: DECIMALS[1],
  drawdown: {
    kind: 1, // Trailing
    limit: DECIMALS[2],
    floor: DECIMALS[3],
    hwm: DECIMALS[4],
  },
  defaultLeverage: DECIMALS[5],
  overrides: [
    { assetClass: "BTC", cap: d(5, 0, 0) },
    { assetClass: "ETH", cap: d(5, 0, 0) },
    { assetClass: "OTHER_CRYPTO", cap: d(2, 0, 0) },
  ],
  instruments: ["BTC", "ETH", "SOL", "DOGE"],
  // A `Some` profit target exercises the Option<Decimal> tail's Some arm. The codec is
  // agnostic to the status/target coupling (a producer concern), so a present target on
  // any status is a valid decode; 105000.00.
  profitTarget: d(10500000, 0, 2),
};

// --- shim harness -------------------------------------------------------------

let instance;

// Writes the JS bytes into guest memory at a freshly allocated pointer and decodes them
// with the production decoder. Returns 1 on success, or -1 for the null result. Memory is
// re-read after alloc in case the stub allocator grew (and detached) it.
function decodeContext(bytes) {
  const ptr = instance.exports.alloc(bytes.length);
  new Uint8Array(instance.exports.memory.buffer).set(Uint8Array.from(bytes), ptr);
  return instance.exports.decode(ptr, bytes.length);
}

// Reads `len` bytes at the absolute `ptr` from the (possibly grown) guest memory and
// decodes them as UTF-8 — used for the aliased asset-class and instrument byte slices.
function readString(ptr, len) {
  const view = new Uint8Array(instance.exports.memory.buffer, Number(ptr), len);
  return new TextDecoder().decode(view);
}

// Asserts the `field`-th top-level decimal equals the fixture: both i128 mantissa halves
// and the scale.
function assertDecimal(field, want, label) {
  assert.equal(instance.exports.decLow(field), want.lo, `${label}.low`);
  assert.equal(instance.exports.decHigh(field), want.hi, `${label}.high`);
  assert.equal(instance.exports.decScale(field), want.scale, `${label}.scale`);
}

before(() => {
  // Build the test-only context decoder shim with the pinned asc and an explicit config so
  // the root asconfig.json (the sample) is not auto-merged.
  execFileSync(
    ASC_BIN,
    ["--config", "asconfig.account-context-shim.json", "--outFile", "build/account-context-shim.wasm"],
    { cwd: PKG_ROOT, stdio: "pipe" }
  );
});

before(async () => {
  const module = new WebAssembly.Module(await readFile(SHIM_WASM));
  // env.abort is never reached (total decoders + --noAssert); a stub satisfies the import.
  instance = new WebAssembly.Instance(module, { env: { abort: () => {} } });
});

// --- tests --------------------------------------------------------------------

test("decodes a full context and round-trips every scalar field and the drawdown sub-shape", () => {
  assert.equal(decodeContext(contextBytes(FULL_CONTEXT)), 1, "a well-formed context must decode");

  assert.equal(instance.exports.status(), FULL_CONTEXT.status, "status discriminant");
  assert.equal(instance.exports.drawdownKind(), FULL_CONTEXT.drawdown.kind, "drawdown kind discriminant");

  const LABELS = [
    "dailyLossLimit",
    "dailyLossFloor",
    "drawdown.limit",
    "drawdown.floor",
    "drawdown.highWaterMark",
    "defaultLeverage",
  ];
  for (let field = 0; field < DECIMALS.length; field++) {
    assertDecimal(field, DECIMALS[field], LABELS[field]);
  }

  // The Option<Decimal> profit-target tail's Some arm round-trips to the exact decimal.
  assert.equal(instance.exports.profitTargetPresent(), 1, "the Some profit target is present");
  assert.equal(instance.exports.profitTargetLow(), FULL_CONTEXT.profitTarget.lo, "profitTarget.low");
  assert.equal(instance.exports.profitTargetHigh(), FULL_CONTEXT.profitTarget.hi, "profitTarget.high");
  assert.equal(instance.exports.profitTargetScale(), FULL_CONTEXT.profitTarget.scale, "profitTarget.scale");
});

test("decodes the leverage-overrides list: each asset class and cap round-trips", () => {
  decodeContext(contextBytes(FULL_CONTEXT));
  const want = FULL_CONTEXT.overrides;
  assert.equal(instance.exports.overrideCount(), want.length, "override count");
  for (let i = 0; i < want.length; i++) {
    const ptr = instance.exports.overrideAssetPtr(i);
    const len = instance.exports.overrideAssetLen(i);
    assert.equal(readString(ptr, len), want[i].assetClass, `override[${i}].assetClass`);
    assert.equal(instance.exports.overrideCapLow(i), want[i].cap.lo, `override[${i}].cap.low`);
    assert.equal(instance.exports.overrideCapHigh(i), want[i].cap.hi, `override[${i}].cap.high`);
    assert.equal(instance.exports.overrideCapScale(i), want[i].cap.scale, `override[${i}].cap.scale`);
  }
});

test("decodes the allowed-instruments list: each symbol round-trips", () => {
  decodeContext(contextBytes(FULL_CONTEXT));
  const want = FULL_CONTEXT.instruments;
  assert.equal(instance.exports.instrumentCount(), want.length, "instrument count");
  for (let i = 0; i < want.length; i++) {
    const ptr = instance.exports.instrumentPtr(i);
    const len = instance.exports.instrumentLen(i);
    assert.equal(readString(ptr, len), want[i], `instrument[${i}]`);
  }
});

test("a context with both lists empty decodes successfully with zero-length lists", () => {
  const empty = {
    status: 0, // Evaluation
    dailyLossLimit: d(100, 0, 0),
    dailyLossFloor: d(50, 0, 0),
    drawdown: { kind: 0, limit: d(10, 0, 0), floor: d(40, 0, 0), hwm: d(100, 0, 0) },
    defaultLeverage: d(2, 0, 0),
    overrides: [],
    instruments: [],
    profitTarget: null, // Funded-style: no profit target (None arm of the tail).
  };
  assert.equal(decodeContext(contextBytes(empty)), 1, "empty lists are valid, not a decode failure");
  assert.equal(instance.exports.status(), 0, "status still decodes on empty lists");
  assert.equal(instance.exports.drawdownKind(), 0, "drawdown kind still decodes on empty lists");
  assert.equal(instance.exports.overrideCount(), 0, "no overrides");
  assert.equal(instance.exports.instrumentCount(), 0, "no instruments");
  assert.equal(instance.exports.profitTargetPresent(), 0, "a None profit target decodes to null");
});

test("a full-capacity context (64 overrides, 1024 instruments) decodes every element", () => {
  const overrides = Array.from({ length: MAX_LEVERAGE_OVERRIDE_COUNT }, (_, i) => ({
    assetClass: `AC${String(i).padStart(3, "0")}`,
    cap: d(i + 1, 0, 1),
  }));
  const instruments = Array.from({ length: MAX_ALLOWED_INSTRUMENT_COUNT }, (_, i) => `S${String(i).padStart(4, "0")}`);
  const ctx = { ...FULL_CONTEXT, overrides, instruments };

  assert.equal(decodeContext(contextBytes(ctx)), 1, "a context at both caps must decode");
  assert.equal(instance.exports.overrideCount(), MAX_LEVERAGE_OVERRIDE_COUNT);
  assert.equal(instance.exports.instrumentCount(), MAX_ALLOWED_INSTRUMENT_COUNT);
  // First, last, and an interior element of each list.
  for (const i of [0, 31, MAX_LEVERAGE_OVERRIDE_COUNT - 1]) {
    const ptr = instance.exports.overrideAssetPtr(i);
    const len = instance.exports.overrideAssetLen(i);
    assert.equal(readString(ptr, len), overrides[i].assetClass, `override[${i}].assetClass`);
    assert.equal(instance.exports.overrideCapLow(i), overrides[i].cap.lo, `override[${i}].cap.low`);
  }
  for (const i of [0, 512, MAX_ALLOWED_INSTRUMENT_COUNT - 1]) {
    const ptr = instance.exports.instrumentPtr(i);
    const len = instance.exports.instrumentLen(i);
    assert.equal(readString(ptr, len), instruments[i], `instrument[${i}]`);
  }
});

test("an over-cap override count (65) yields the no-context result", () => {
  // Header through default_leverage, then an over-cap override count with no override
  // bytes: the decoder rejects it before trusting the count for sizing, exactly as the
  // host does. decode() returns -1 (null).
  const bytes = [
    0, // status
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    0, // drawdown kind
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...u32le(MAX_LEVERAGE_OVERRIDE_COUNT + 1),
  ];
  assert.equal(decodeContext(bytes), -1, "an over-cap override count must decode to null");
});

test("an over-cap instrument count (1025) yields the no-context result", () => {
  // Valid header with zero overrides, then an over-cap instrument count with no instrument
  // bytes. The decoder rejects it before sizing the list.
  const bytes = [
    0,
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    0,
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...u32le(0), // zero overrides
    ...u32le(MAX_ALLOWED_INSTRUMENT_COUNT + 1),
  ];
  assert.equal(decodeContext(bytes), -1, "an over-cap instrument count must decode to null");
});

test("a buffer truncated mid-header yields the no-context result", () => {
  // Only the status byte and a partial first decimal follow: the fixed-width header reads
  // run past the end and the decoder fails safe.
  const bytes = [0, 1, 2, 3, 4];
  assert.equal(decodeContext(bytes), -1, "a truncated header must decode to null");
});

test("a buffer that does not hold the claimed overrides yields the no-context result", () => {
  // The header promises 2 overrides but only one (a 4-byte asset string + 20-byte cap)
  // plus stray bytes follow.
  const oneOverride = [...strBytes("BTC"), ...decBytes(d(5, 0, 0))];
  const bytes = [
    0,
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    0,
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...decBytes(d(0, 0, 0)),
    ...u32le(2), // claims two overrides
    ...oneOverride,
    1, 2, 3, // stray, not a second override
  ];
  assert.equal(decodeContext(bytes), -1, "a truncated override list must decode to null");
});
