// Public SDK surface, re-exported as a single import point for a bot author —
// mirroring the Rust SDK's `propify-bot-sdk` crate root.
//
// A bot module imports its types and the driver from here, then exports the four
// ABI functions by delegating to the helpers below (see `assembly/sample.ts`).

export { Decimal } from "./decimal";
export { ByteSlice } from "./wire";
export {
  Exchange,
  ProductType,
  OrderSide,
  PositionSide,
  OrderType,
  TimeInForce,
  AccountStatus,
  DrawdownKind,
  MarketSnapshot,
  Candle,
  MarketWindow,
  MAX_CANDLE_COUNT,
  StrategyParams,
  AccountView,
  DrawdownRule,
  LeverageOverride,
  AccountContext,
  MAX_LEVERAGE_OVERRIDE_COUNT,
  MAX_ALLOWED_INSTRUMENT_COUNT,
  OrderIntentBody,
} from "./types";
export { Bot, runTick, abiVersion, alloc, dealloc, ABI_VERSION } from "./bot";
