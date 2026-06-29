// Off-target unit tests for the AssemblyScript MarketWindow DECODER (ABI v2).
//
// The existing sample.test.mjs serves a populated window to the snapshot-only sample,
// but the sample ignores the window, so no EXECUTED assertion ever covered the decoded
// per-candle VALUES — only the empty-window path flowed through. These tests close that
// gap: they compile a tiny test-only shim (test/decoder-shim.ts) that re-exports the
// PRODUCTION MarketWindow.decode + candleAt unchanged, feed it canonical window bytes,
// and assert the asset, the candle count, and every candle's timestamp_ms and 5 OHLCV
// Decimals round-trip to the exact values encoded. Fail-safe cases (over-cap count,
// truncated buffers) must yield the no-window result.
//
// The shim is NOT the sample: it is a separate asc entry built to build/decoder-shim.wasm
// (gitignored), so no production source is touched and the frozen sample module is not
// bloated. The fixture encoders below are hand-written and independent of the SDK codec,
// so a shared bug cannot hide a decoder bug.
//
// Zero test dependencies beyond Node's built-ins. The shim is compiled in a before()
// hook via the pinned asc, so `pnpm test` is self-contained.

import { test, before } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const PKG_ROOT = fileURLToPath(new URL("..", import.meta.url));
const ASC_BIN = fileURLToPath(new URL("../node_modules/.bin/asc", import.meta.url));
const SHIM_WASM = fileURLToPath(new URL("../build/decoder-shim.wasm", import.meta.url));

// --- canonical v2 wire encoders (little-endian), independent of the SDK -------

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

// A candle on the wire: i64 timestamp_ms, then five Decimals (open, high, low, close,
// volume). c.ohlcv is an array of exactly five { lo, hi, scale } in that order.
const candleBytes = (c) => [...i64le(c.ts), ...c.ohlcv.flatMap(decBytes)];

// A MarketWindow: asset (String), a u32 candle count, then that many candles.
const windowBytes = (asset, candles) => {
  const out = [...strBytes(asset), ...u32le(candles.length)];
  for (const c of candles) out.push(...candleBytes(c));
  return out;
};

// --- fixtures: distinct, non-trivial decimals across every field --------------
// Coverage in one window: a small positive (100.5), a tiny fraction (0.002), a
// NEGATIVE mantissa (-123.45, low -12345 / high all-ones -1), a LARGE i128 whose high
// half is non-zero (1<<64 + 7 => low 7 / high 1), an all-ones low half (-1), an
// all-ones high half, the max scale 28, and exact zero. No two fields collide.

const d = (lo, hi, scale) => ({ lo: BigInt(lo), hi: BigInt(hi), scale });

const THREE_CANDLES = [
  {
    ts: 1_699_999_880_000n,
    ohlcv: [
      d(1005, 0, 1), // open   100.5
      d(2, 0, 3), // high   0.002
      d(-12345, -1, 2), // low    -123.45 (i128 -12345)
      d(7, 1, 0), // close  1<<64 + 7 (high half set)
      d(999999, 0, 0), // volume 999999
    ],
  },
  {
    ts: 1_699_999_940_000n,
    ohlcv: [
      d(5005, 0, 2), // open   50.05
      d(9, 0, 4), // high   0.0009
      d(-67890, -1, 1), // low    -6789.0 (i128 -67890)
      d(42, 3, 5), // close  3<<64 + 42
      d(1234567, 0, 0), // volume 1234567
    ],
  },
  {
    ts: 1_700_000_000_000n,
    ohlcv: [
      d(1, 0, 28), // open   1e-28 (max scale)
      d(-1, 0, 0), // high   all-ones low half (i128 2^64 - 1)
      d(0, -1, 0), // low    all-ones high half
      d(250000, 0, 2), // close  2500.00
      d(0, 0, 0), // volume exactly zero
    ],
  },
];

// --- shim harness -------------------------------------------------------------

let instance;

// Reads the JS bytes into guest memory at a freshly allocated pointer and decodes them
// with the production decoder. Returns the candle count, or -1 for the null result.
// Memory is re-read after alloc in case the stub allocator grew (and detached) it.
function decodeWindow(bytes) {
  const ptr = instance.exports.alloc(bytes.length);
  new Uint8Array(instance.exports.memory.buffer).set(Uint8Array.from(bytes), ptr);
  return instance.exports.decode(ptr, bytes.length);
}

function decodedAsset() {
  const ptr = Number(instance.exports.assetPtr());
  const len = instance.exports.assetLen();
  const view = new Uint8Array(instance.exports.memory.buffer, ptr, len);
  return new TextDecoder().decode(view);
}

// Asserts the decoded candle at index i equals the fixture: timestamp and all five
// Decimals (both i128 mantissa halves and the scale), read in place via candleAt.
function assertCandle(i, want) {
  assert.equal(instance.exports.candleTs(i), want.ts, `candle[${i}].ts`);
  const FIELDS = ["open", "high", "low", "close", "volume"];
  for (let f = 0; f < 5; f++) {
    const e = want.ohlcv[f];
    assert.equal(instance.exports.candleDecLow(i, f), e.lo, `candle[${i}].${FIELDS[f]}.low`);
    assert.equal(instance.exports.candleDecHigh(i, f), e.hi, `candle[${i}].${FIELDS[f]}.high`);
    assert.equal(instance.exports.candleDecScale(i, f), e.scale, `candle[${i}].${FIELDS[f]}.scale`);
  }
}

before(() => {
  // Build the test-only decoder shim with the pinned asc and an explicit config so the
  // root asconfig.json (the sample) is not auto-merged.
  execFileSync(ASC_BIN, ["--config", "asconfig.decoder-shim.json", "--outFile", "build/decoder-shim.wasm"], {
    cwd: PKG_ROOT,
    stdio: "pipe",
  });
});

before(async () => {
  const module = new WebAssembly.Module(await readFile(SHIM_WASM));
  // env.abort is never reached (total decoders + --noAssert); a stub satisfies the import.
  instance = new WebAssembly.Instance(module, { env: { abort: () => {} } });
});

// --- tests --------------------------------------------------------------------

test("decodes a populated window and round-trips every candle field exactly", () => {
  const count = decodeWindow(windowBytes("BTC", THREE_CANDLES));
  assert.equal(count, THREE_CANDLES.length, "decode must report the candle count");
  assert.equal(decodedAsset(), "BTC", "decoded asset must match");
  for (let i = 0; i < THREE_CANDLES.length; i++) assertCandle(i, THREE_CANDLES[i]);
});

test("an empty (warm-up) window decodes successfully with zero candles", () => {
  const count = decodeWindow(windowBytes("ETH", []));
  assert.equal(count, 0, "an empty window is valid (count 0), not a decode failure");
  assert.equal(decodedAsset(), "ETH", "asset must still decode on an empty window");
});

test("a full 256-candle window decodes with every candle readable in place", () => {
  const MAX = 256;
  const candles = Array.from({ length: MAX }, (_, i) => {
    const v = i + 1;
    return {
      ts: BigInt(1_700_000_000_000 + i),
      ohlcv: [
        d(v, 0, 1),
        d(v * 2, 0, 2),
        d(-v, -1, 3), // negative i128 -v
        d(v, 1, 4), // high half set
        d(v * 10, 0, 0),
      ],
    };
  });
  const count = decodeWindow(windowBytes("SOL", candles));
  assert.equal(count, MAX, "a full window at the cap must decode all 256 candles");
  assert.equal(decodedAsset(), "SOL");
  // First, last, and an interior candle: candleAt reads each at a constant offset.
  assertCandle(0, candles[0]);
  assertCandle(128, candles[128]);
  assertCandle(MAX - 1, candles[MAX - 1]);
});

test("a count above MAX_CANDLE_COUNT (257) yields the no-window result", () => {
  // Over-cap count with no candle bytes: the decoder rejects it before trusting the
  // count for bounds, exactly as the host does. decode() returns -1 (null).
  const bytes = [...strBytes("BTC"), ...u32le(257)];
  assert.equal(decodeWindow(bytes), -1, "an over-cap count must decode to null");
});

test("a buffer that does not hold the claimed candles yields the no-window result", () => {
  // The header promises 2 candles but only one (108 bytes) plus stray bytes follow.
  const oneCandle = candleBytes(THREE_CANDLES[0]);
  assert.equal(oneCandle.length, 108, "a candle is 108 wire bytes");
  const bytes = [...strBytes("BTC"), ...u32le(2), ...oneCandle, 1, 2, 3];
  assert.equal(decodeWindow(bytes), -1, "a truncated candle array must decode to null");
});

test("a truncated asset header yields the no-window result", () => {
  // The asset length prefix claims 5 bytes but none follow.
  const bytes = [0x05, 0x00, 0x00, 0x00];
  assert.equal(decodeWindow(bytes), -1, "a truncated header must decode to null");
});
