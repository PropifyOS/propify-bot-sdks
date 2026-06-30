// A trivial, fully deterministic sample bot — the AssemblyScript counterpart of the
// Rust `propify-bot-sdk-sample`.
//
// It reads the market snapshot and the strategy parameters, then emits one fixed
// market BUY for the snapshot's asset. It exists to exercise the SDK and to be the
// fixture the host's acceptance test loads (compiled to `build/sample.wasm`). It is
// NOT a product strategy.
//
// The rule is a pure function of the inputs — no clock, no randomness, no `f64`: buy
// `quantity` units of `market.asset`, where `quantity` is the `"quantity"` strategy
// parameter if present, else a fixed default of `0.001`. Identical inputs always
// produce identical emitted bytes.
//
// This module is the compiled entry. It implements the bot and exports the four
// ABI functions by delegating to the SDK helpers — the AS equivalent of Rust's
// `register_bot!` macro, written by hand because AS has no macros. The linear
// `memory` export is emitted automatically (we do not pass `--importMemory`).

// Pull the no-op `abort` into the compilation so `--use abort=assembly/noop/...`
// can resolve it. Without this side-effect import the file is not compiled and the
// override target cannot be found.
import "./noop";

import {
  Bot,
  MarketSnapshot,
  MarketWindow,
  StrategyParams,
  AccountView,
  AccountContext,
  OrderIntentBody,
  Decimal,
  Exchange,
  ProductType,
  OrderSide,
  PositionSide,
  OrderType,
  TimeInForce,
  runTick,
  abiVersion as sdkAbiVersion,
  alloc as sdkAlloc,
  dealloc as sdkDealloc,
} from "./index";

/**
 * The ASCII bytes of the parameter name `"quantity"`, held as a static data segment
 * (no runtime allocation) so the lookup is a plain byte comparison with no UTF-8
 * decoding. q,u,a,n,t,i,t,y = 113,117,97,110,116,105,116,121.
 */
// prettier-ignore
const QUANTITY_NAME: StaticArray<u8> = [113, 117, 97, 110, 116, 105, 116, 121];

/**
 * The order size used when the strategy parameters do not supply a `"quantity"`:
 * `0.001`, built from an exact `(mantissa, scale) = (1, 3)` so no `f64` is involved.
 */
function defaultQuantity(): Decimal {
  return Decimal.fromI64(1, 3);
}

/**
 * The sample strategy. Stateless: the host re-instantiates the guest each tick, so
 * the decision depends only on this tick's inputs.
 */
class SampleBot extends Bot {
  onTick(
    market: MarketSnapshot,
    window: MarketWindow,
    params: StrategyParams,
    account: AccountView,
    context: AccountContext
  ): OrderIntentBody | null {
    // The sample is a snapshot-only bot: it ignores the ABI v2 window and the ABI v3
    // account context and decides from the latest candle alone. A real bot would consult
    // `context` (the drawdown floor, leverage caps) to shape its own behaviour.
    // Take the order size from the `"quantity"` parameter when present, else the
    // fixed default. Looking it up by name keeps the bot deterministic and
    // independent of parameter ordering.
    const found = params.find(QUANTITY_NAME);
    const quantity = found !== null ? found : defaultQuantity();

    return new OrderIntentBody(
      Exchange.Hyperliquid,
      market.asset, // pass the snapshot's symbol straight through, no copy
      ProductType.Perp,
      OrderSide.Buy,
      PositionSide.Long,
      OrderType.Market,
      TimeInForce.Ioc,
      quantity,
      null, // price: market order carries none
      null, // trigger_price: none
      false // reduce_only
    );
  }
}

// --- ABI exports (the hand-written equivalent of `register_bot!`) ----------

export function abi_version(): i32 {
  return sdkAbiVersion();
}

export function alloc(size: i32): i32 {
  return sdkAlloc(size);
}

export function dealloc(ptr: i32, size: i32): void {
  sdkDealloc(ptr, size);
}

export function on_tick(): void {
  // Fresh per tick: the host re-instantiates the module each tick, so there is no
  // state to carry over.
  runTick(new SampleBot());
}
