// Behavioural tests for the compiled AssemblyScript sample guest.
//
// These instantiate `build/sample.wasm` with a mock `propify` host that mirrors the
// Rust host's read/emit protocol byte-for-byte (the too-small-buffer retry
// included), drive `on_tick`, and assert the emitted `OrderIntentBody` bytes equal a
// hand-built expectation. This is the cross-language correctness crux: if the AS
// encoder and the Rust codec disagree on a single byte, these fail.
//
// Zero test dependencies: Node's built-in `node:test` + `node:assert`. Run after
// `pnpm run build:sample` (the wasm must exist).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const WASM_PATH = fileURLToPath(new URL("../build/sample.wasm", import.meta.url));

// --- v1 wire encoders (little-endian), independent of the SDK -----------------
// A deliberately separate, hand-written encoder: the test must not reuse the code
// under test, or a shared bug would hide. These mirror the spec's wire format.

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
const decBytes = (lo, hi, scale) => [...i64le(lo), ...i64le(hi), ...i32le(scale)];

const marketBytes = (asset, tsMs, prices) => [
  ...strBytes(asset),
  ...i64le(tsMs),
  ...prices.flatMap((p) => decBytes(p[0], p[1], p[2])),
];

const paramsBytes = (pairs) => {
  const out = [...u32le(pairs.length)];
  for (const [name, d] of pairs) {
    out.push(...strBytes(name), ...decBytes(d[0], d[1], d[2]));
  }
  return out;
};

const accountBytes = (four) => four.flatMap((d) => decBytes(d[0], d[1], d[2]));

// An ABI v2 candle on the wire: i64 timestamp_ms, then five Decimals (OHLCV).
const candleBytes = (tsMs, prices) => [...i64le(tsMs), ...prices.flatMap((p) => decBytes(p[0], p[1], p[2]))];

// An ABI v2 MarketWindow: asset (String), a u32 candle count, then that many candles.
const windowBytes = (asset, candles) => {
  const out = [...strBytes(asset), ...u32le(candles.length)];
  for (const c of candles) out.push(...candleBytes(c.ts, c.prices));
  return out;
};

// The empty window the host serves for a v1 tick or during live warm-up: asset "" and
// zero candles. A snapshot-only bot reads it and ignores it, so the emission is
// unchanged. This is the default the mock serves unless a test supplies its own window.
const EMPTY_WINDOW = windowBytes("", []);

const orderBytes = (o) => {
  const out = [
    o.exchange,
    ...strBytes(o.asset),
    o.product,
    o.side,
    o.pos,
    o.otype,
    o.tif,
    ...decBytes(o.qty[0], o.qty[1], o.qty[2]),
  ];
  out.push(...(o.price === null ? [0] : [1, ...decBytes(o.price[0], o.price[1], o.price[2])]));
  out.push(...(o.trigger === null ? [0] : [1, ...decBytes(o.trigger[0], o.trigger[1], o.trigger[2])]));
  out.push(o.reduce ? 1 : 0);
  return out;
};

// --- Mock host ----------------------------------------------------------------

/** A representative account view; the sample ignores it but must still read it. */
const SAMPLE_ACCOUNT = accountBytes([
  [100000n, 0n, 2], // equity 1000.00
  [50000n, 0n, 2], // available_margin 500.00
  [0n, 0n, 0], // exposure 0
  [0n, 0n, 0], // unrealized_pnl 0
]);

/** Five plausible prices; their exact values do not affect the emission. */
const SAMPLE_PRICES = [
  [9500000n, 0n, 2],
  [9550000n, 0n, 2],
  [9480000n, 0n, 2],
  [9520050n, 0n, 2],
  [12345n, 0n, 1],
];

/**
 * Instantiates the module with a mock host and runs one tick.
 *
 * The read functions implement the host protocol: write the payload into guest
 * memory when it fits, otherwise write nothing and return the required length so the
 * guest re-allocs and retries. `host_emit_intent` captures the emitted bytes.
 */
function runTick(module, market, params, account, window = EMPTY_WINDOW) {
  let emitted = null;
  let instance;

  const memU8 = () => new Uint8Array(instance.exports.memory.buffer);
  const serve = (payload) => (ptr, len) => {
    if (len < payload.length) return payload.length; // too small: write nothing
    memU8().set(Uint8Array.from(payload), ptr);
    return payload.length;
  };

  const importObject = {
    propify: {
      host_read_market_data: serve(market),
      // ABI v2: the host serves the bounded candle window. The snapshot-only sample
      // reads it (so the import must be present) and ignores it.
      host_read_market_window: serve(window),
      host_read_strategy_params: serve(params),
      host_read_account_view: serve(account),
      host_emit_intent: (ptr, len) => {
        emitted = Array.from(memU8().subarray(ptr, ptr + len));
        return 0;
      },
    },
  };

  instance = new WebAssembly.Instance(module, importObject);
  instance.exports.on_tick();
  return { emitted, instance };
}

async function loadModule() {
  return new WebAssembly.Module(await readFile(WASM_PATH));
}

// --- Tests --------------------------------------------------------------------

test("imports exactly the five propify capabilities and nothing else", async () => {
  const module = await loadModule();
  const imports = WebAssembly.Module.imports(module);
  const names = imports.map((i) => `${i.module}::${i.name}`).sort();
  // Five under ABI v2: the four v1 reads/emit plus the new market-window read.
  assert.deepEqual(names, [
    "propify::host_emit_intent",
    "propify::host_read_account_view",
    "propify::host_read_market_data",
    "propify::host_read_market_window",
    "propify::host_read_strategy_params",
  ]);
  // Every import is a function in the `propify` module: no env.abort/seed/memory.
  for (const imp of imports) {
    assert.equal(imp.module, "propify");
    assert.equal(imp.kind, "function");
  }
});

test("exports the full ABI surface", async () => {
  const module = await loadModule();
  const exports = WebAssembly.Module.exports(module);
  const byName = new Map(exports.map((e) => [e.name, e.kind]));
  assert.equal(byName.get("memory"), "memory");
  assert.equal(byName.get("abi_version"), "function");
  assert.equal(byName.get("alloc"), "function");
  assert.equal(byName.get("dealloc"), "function");
  assert.equal(byName.get("on_tick"), "function");
});

test("abi_version returns the ABI v2 value 2", async () => {
  const module = await loadModule();
  const instance = new WebAssembly.Instance(module, {
    propify: {
      host_read_market_data: () => 0,
      host_read_market_window: () => 0,
      host_read_strategy_params: () => 0,
      host_read_account_view: () => 0,
      host_emit_intent: () => 0,
    },
  });
  assert.equal(instance.exports.abi_version(), 2);
});

test("emits the exact market-BUY bytes for asset BTC and quantity 0.002", async () => {
  const module = await loadModule();
  const market = marketBytes("BTC", 1_700_000_000_000n, SAMPLE_PRICES);
  // quantity = 0.002 -> mantissa 2, scale 3 (the acceptance fixture).
  const params = paramsBytes([["quantity", [2n, 0n, 3]]]);
  const { emitted } = runTick(module, market, params, SAMPLE_ACCOUNT);

  const expected = orderBytes({
    exchange: 0, // Hyperliquid
    asset: "BTC",
    product: 1, // Perp
    side: 0, // Buy
    pos: 0, // Long
    otype: 0, // Market
    tif: 1, // Ioc
    qty: [2n, 0n, 3], // 0.002
    price: null,
    trigger: null,
    reduce: false,
  });

  assert.notEqual(emitted, null, "the bot must emit one intent");
  assert.deepEqual(emitted, expected, "emitted wire bytes must match the Rust codec");
  // 36 bytes: 1 + (4+3) + 1 + 1 + 1 + 1 + 1 + 20 + 1 + 1 + 1.
  assert.equal(emitted.length, 36);
});

test("falls back to the default quantity 0.001 when no quantity param is present", async () => {
  const module = await loadModule();
  const market = marketBytes("ETH", 1_700_000_000_000n, SAMPLE_PRICES);
  const params = paramsBytes([]); // empty param list
  const { emitted } = runTick(module, market, params, SAMPLE_ACCOUNT);

  const expected = orderBytes({
    exchange: 0,
    asset: "ETH",
    product: 1,
    side: 0,
    pos: 0,
    otype: 0,
    tif: 1,
    qty: [1n, 0n, 3], // default 0.001
    price: null,
    trigger: null,
    reduce: false,
  });
  assert.deepEqual(emitted, expected);
});

test("looks the quantity param up by name regardless of order", async () => {
  const module = await loadModule();
  const market = marketBytes("BTC", 1n, SAMPLE_PRICES);
  // Decoy params before and after the real one; the lookup is by name, not position.
  const params = paramsBytes([
    ["fast", [12n, 0n, 0]],
    ["quantity", [5n, 0n, 1]], // 0.5
    ["slow", [26n, 0n, 0]],
  ]);
  const { emitted } = runTick(module, market, params, SAMPLE_ACCOUNT);
  const expected = orderBytes({
    exchange: 0,
    asset: "BTC",
    product: 1,
    side: 0,
    pos: 0,
    otype: 0,
    tif: 1,
    qty: [5n, 0n, 1], // 0.5
    price: null,
    trigger: null,
    reduce: false,
  });
  assert.deepEqual(emitted, expected);
});

test("recovers via the single-retry path when the first read buffer is too small", async () => {
  const module = await loadModule();
  // An asset longer than the 256-byte initial read buffer forces the host to return
  // the required length on the first call (writing nothing) and the guest to re-alloc
  // and retry once.
  const longAsset = "A".repeat(300);
  const market = marketBytes(longAsset, 1n, SAMPLE_PRICES);
  const params = paramsBytes([["quantity", [2n, 0n, 3]]]);
  const { emitted } = runTick(module, market, params, SAMPLE_ACCOUNT);

  const expected = orderBytes({
    exchange: 0,
    asset: longAsset,
    product: 1,
    side: 0,
    pos: 0,
    otype: 0,
    tif: 1,
    qty: [2n, 0n, 3],
    price: null,
    trigger: null,
    reduce: false,
  });
  assert.deepEqual(emitted, expected, "the retry path must recover the full snapshot");
});

test("is deterministic: identical inputs produce identical emitted bytes", async () => {
  const module = await loadModule();
  const market = marketBytes("BTC", 1_700_000_000_000n, SAMPLE_PRICES);
  const params = paramsBytes([["quantity", [2n, 0n, 3]]]);
  const a = runTick(module, market, params, SAMPLE_ACCOUNT).emitted;
  const b = runTick(module, market, params, SAMPLE_ACCOUNT).emitted;
  assert.deepEqual(a, b);
});

test("serves a populated window yet the snapshot-only sample emits unchanged", async () => {
  // A non-empty, two-candle window is served. The sample ignores the window and decides
  // from the snapshot alone, so the emitted bytes must be byte-identical to the
  // empty-window case: proving the ABI v2 window read does not disturb a window-unaware
  // bot, the AS counterpart of the Rust `run_tick_serves_a_window_yet_a_snapshot_only_bot`.
  const module = await loadModule();
  const market = marketBytes("BTC", 1_700_000_000_000n, SAMPLE_PRICES);
  const params = paramsBytes([["quantity", [2n, 0n, 3]]]);
  const window = windowBytes("BTC", [
    { ts: 1_699_999_940_000n, prices: SAMPLE_PRICES },
    { ts: 1_700_000_000_000n, prices: SAMPLE_PRICES },
  ]);
  const withWindow = runTick(module, market, params, SAMPLE_ACCOUNT, window).emitted;
  const withEmpty = runTick(module, market, params, SAMPLE_ACCOUNT, EMPTY_WINDOW).emitted;

  const expected = orderBytes({
    exchange: 0,
    asset: "BTC",
    product: 1,
    side: 0,
    pos: 0,
    otype: 0,
    tif: 1,
    qty: [2n, 0n, 3],
    price: null,
    trigger: null,
    reduce: false,
  });
  assert.deepEqual(withWindow, expected, "a populated window must not change the emission");
  assert.deepEqual(withWindow, withEmpty, "window contents must not affect a snapshot-only bot");
});

test("recovers via the single-retry path when the first window buffer is too small", async () => {
  // A window with enough candles to exceed the 256-byte initial read buffer forces the
  // host to return the required length on the first window read (writing nothing) and the
  // guest to re-alloc and retry once — the same protocol as every other read. The sample
  // ignores the window, so a correct emission proves the window retry path recovered
  // cleanly without corrupting the tick.
  const module = await loadModule();
  const market = marketBytes("BTC", 1n, SAMPLE_PRICES);
  const params = paramsBytes([["quantity", [2n, 0n, 3]]]);
  // 3 candles * 108 bytes + header > 256, so the first 256-byte read cannot hold it.
  const candles = Array.from({ length: 3 }, (_, i) => ({ ts: BigInt(i), prices: SAMPLE_PRICES }));
  const bigWindow = windowBytes("BTC", candles);
  assert.ok(bigWindow.length > 256, "the window must exceed the initial read buffer");
  const { emitted } = runTick(module, market, params, SAMPLE_ACCOUNT, bigWindow);

  const expected = orderBytes({
    exchange: 0,
    asset: "BTC",
    product: 1,
    side: 0,
    pos: 0,
    otype: 0,
    tif: 1,
    qty: [2n, 0n, 3],
    price: null,
    trigger: null,
    reduce: false,
  });
  assert.deepEqual(emitted, expected, "the window retry path must not disturb the emission");
});

test("does not emit when a read returns the internal host error -1", async () => {
  const module = await loadModule();
  let emitted = null;
  let instance;
  const memU8 = () => new Uint8Array(instance.exports.memory.buffer);
  instance = new WebAssembly.Instance(module, {
    propify: {
      host_read_market_data: () => -1, // internal host error
      host_read_market_window: () => 0,
      host_read_strategy_params: () => 0,
      host_read_account_view: () => 0,
      host_emit_intent: (ptr, len) => {
        emitted = Array.from(memU8().subarray(ptr, ptr + len));
        return 0;
      },
    },
  });
  instance.exports.on_tick();
  assert.equal(emitted, null, "a -1 read error must abort the tick with no emission");
});
