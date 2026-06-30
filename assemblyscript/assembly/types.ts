// The decoded boundary view types a bot reads, the body it builds, and the six
// closed enums — mirroring the Rust SDK's re-exported `propify-sandbox-abi` DTOs so
// the creator's mental model is identical across languages.
//
// Enum discriminants are FROZEN and match the Rust codec exactly (see
// `propify-sandbox-abi::wire`). AS has no first-class enum that pins a wire byte, so
// each is a `namespace` of `u8` constants; the encoder writes the byte directly.

import { Decimal } from "./decimal";
import { ByteSlice, Reader, sliceEqualsAscii } from "./wire";

export namespace Exchange {
  export const Hyperliquid: u8 = 0;
}

export namespace ProductType {
  export const Spot: u8 = 0;
  export const Perp: u8 = 1;
}

export namespace OrderSide {
  export const Buy: u8 = 0;
  export const Sell: u8 = 1;
}

export namespace PositionSide {
  export const Long: u8 = 0;
  export const Short: u8 = 1;
}

export namespace OrderType {
  export const Market: u8 = 0;
  export const Limit: u8 = 1;
  export const StopMarket: u8 = 2;
  export const StopLimit: u8 = 3;
  export const TakeProfitMarket: u8 = 4;
  export const TakeProfitLimit: u8 = 5;
}

export namespace TimeInForce {
  export const Gtc: u8 = 0;
  export const Ioc: u8 = 1;
  export const Fok: u8 = 2;
  export const Gtx: u8 = 3;
}

/**
 * Account lifecycle status (ABI v3, FROZEN). Resolved host-side from the challenge
 * attempt and carried in an [`AccountContext`]. `Evaluation` is an account still
 * working a challenge; `Funded` is a passed, funded account.
 */
export namespace AccountStatus {
  export const Evaluation: u8 = 0;
  export const Funded: u8 = 1;
}

/**
 * How the maximum-drawdown floor is computed (ABI v3, FROZEN). `Static` is a fixed
 * anchor minus the limit; `Trailing` rises with the high-water mark. The host resolves
 * the kind from the challenge tier (1-step is static, 2-step is trailing).
 */
export namespace DrawdownKind {
  export const Static: u8 = 0;
  export const Trailing: u8 = 1;
}

/**
 * A single market observation handed to the bot each tick.
 *
 * `asset` is a [`ByteSlice`] aliasing the host-written input buffer rather than a
 * copied `String`, so a bot can pass the symbol straight through into an order with
 * no allocation and no UTF-8 round-trip. `timestampMs` is the only clock the guest
 * ever sees.
 */
export class MarketSnapshot {
  constructor(
    public asset: ByteSlice,
    public timestampMs: i64,
    public open: Decimal,
    public high: Decimal,
    public low: Decimal,
    public close: Decimal,
    public volume: Decimal
  ) {}

  /**
   * Decodes a snapshot from the wire bytes at `(ptr, len)`. Returns `null` if the
   * buffer is truncated, so the driver can skip the tick rather than act on garbage.
   */
  static decode(ptr: usize, len: i32): MarketSnapshot | null {
    const reader = new Reader(ptr, len);
    const asset = reader.readString();
    const timestampMs = reader.readI64();
    const open = reader.readDecimal();
    const high = reader.readDecimal();
    const low = reader.readDecimal();
    const close = reader.readDecimal();
    const volume = reader.readDecimal();
    if (reader.failed) return null;
    return new MarketSnapshot(asset, timestampMs, open, high, low, close, volume);
  }
}

/**
 * Cap on the number of candles in a [`MarketWindow`] (ABI v2), matching the host's
 * `MAX_CANDLE_COUNT` (`propify-sandbox-abi`). A window claiming more than this is
 * malformed and decodes to `null`, exactly as the host rejects an over-cap count.
 */
export const MAX_CANDLE_COUNT: i32 = 256;

/**
 * Fixed wire size of one candle: an i64 `timestampMs` (8 bytes) plus five `Decimal`s
 * (20 bytes each). Candles are fixed-width, so the i-th candle sits at a constant
 * offset and can be read in place without scanning from the front.
 */
const CANDLE_BYTES: i32 = 8 + 5 * 20;

/**
 * One candle in a [`MarketWindow`]: the OHLCV-plus-`timestampMs` fields of a
 * [`MarketSnapshot`] minus the asset (the asset is carried once on the window). Money
 * is the exact `Decimal`, never `f64`.
 */
export class Candle {
  constructor(
    public timestampMs: i64,
    public open: Decimal,
    public high: Decimal,
    public low: Decimal,
    public close: Decimal,
    public volume: Decimal
  ) {}
}

/**
 * The ABI v2 multi-candle market window: the asset plus a bounded, time-ordered array
 * of recent candles, oldest to newest, ending with the latest. A window-aware bot
 * recomputes a multi-candle indicator from this host-supplied history each tick rather
 * than carrying state across ticks, which keeps it stateless and deterministic.
 *
 * Decoding is copy-free, mirroring [`StrategyParams`]: the view records where the
 * fixed-width candle array begins and reads the i-th candle in place via
 * [`MarketWindow#candleAt`]. During live warm-up, before `MAX_CANDLE_COUNT` candles
 * exist, the window is naturally shorter (and may be empty, `count == 0`); that is a
 * valid window the bot must tolerate, not a decode failure.
 */
export class MarketWindow {
  constructor(
    /** The asset every candle is for, aliasing the source buffer (no copy). */
    public asset: ByteSlice,
    /** Absolute pointer to the first candle's bytes in linear memory. */
    private candlesBase: usize,
    /** Number of candles in the window (`0` during warm-up). */
    public count: i32
  ) {}

  /**
   * Decodes a window from v2 wire bytes at `(ptr, len)`: the `asset` string, a `u32`
   * candle count, then `count` fixed-width candles. Returns `null` — so the driver
   * skips the tick rather than act on garbage — when the buffer is truncated, the
   * count exceeds [`MAX_CANDLE_COUNT`], or the buffer does not actually hold `count`
   * candles. An empty (`count == 0`) window decodes successfully (the warm-up path).
   */
  static decode(ptr: usize, len: i32): MarketWindow | null {
    const reader = new Reader(ptr, len);
    const asset = reader.readString();
    const count = reader.readU32();
    if (reader.failed) return null;
    // Reject an over-cap count exactly as the host does, before trusting it for bounds.
    if (count > <u32>MAX_CANDLE_COUNT) return null;
    const candlesBase = reader.position;
    // Verify the buffer truly holds `count` fixed-width candles. The product is taken
    // in i64 to avoid overflow before the comparison (count is already <= 256 here).
    const needed: i64 = <i64>count * <i64>CANDLE_BYTES;
    if (<i64>reader.remaining < needed) return null;
    return new MarketWindow(asset, candlesBase, <i32>count);
  }

  /**
   * Reads the candle at index `i` (`0` is oldest, `count - 1` is the latest) in place
   * from the source buffer. The caller must pass `0 <= i < count`; the bounded reader
   * over the candle's fixed span keeps an out-of-range index from reading past it.
   */
  candleAt(i: i32): Candle {
    const base = this.candlesBase + <usize>(i * CANDLE_BYTES);
    const reader = new Reader(base, CANDLE_BYTES);
    const timestampMs = reader.readI64();
    const open = reader.readDecimal();
    const high = reader.readDecimal();
    const low = reader.readDecimal();
    const close = reader.readDecimal();
    const volume = reader.readDecimal();
    return new Candle(timestampMs, open, high, low, close, volume);
  }
}

/**
 * The read-only strategy parameters for this tick: a count-prefixed list of
 * `(name, value)` pairs.
 *
 * Decoding is lazy and copy-free: the view holds the raw buffer and scans it on
 * demand in [`StrategyParams#find`], mirroring the Rust SDK's
 * `params.iter().find(name == ...)`. Scanning avoids materializing an array of
 * `String`s (which would pull UTF-8 machinery) and keeps lookups independent of
 * parameter order.
 */
export class StrategyParams {
  private bufPtr: usize;
  private bufLen: i32;

  constructor(ptr: usize, len: i32) {
    this.bufPtr = ptr;
    this.bufLen = len;
  }

  /**
   * Returns the `Decimal` value of the first parameter whose name equals the given
   * ASCII bytes, or `null` if absent. Byte-compares names without UTF-8 decoding.
   */
  find(name: StaticArray<u8>): Decimal | null {
    const reader = new Reader(this.bufPtr, this.bufLen);
    const count = reader.readU32();
    if (reader.failed) return null;
    for (let i: u32 = 0; i < count; i++) {
      const nameSlice = reader.readString();
      const value = reader.readDecimal();
      if (reader.failed) return null;
      if (sliceEqualsAscii(nameSlice, name)) return value;
    }
    return null;
  }
}

/**
 * This account's own figures — the only account data the guest may read. There is
 * deliberately no account id and no peer data on the wire.
 */
export class AccountView {
  constructor(
    public equity: Decimal,
    public availableMargin: Decimal,
    public exposure: Decimal,
    public unrealizedPnl: Decimal
  ) {}

  /** Decodes an account view from the wire bytes, or `null` on a truncated buffer. */
  static decode(ptr: usize, len: i32): AccountView | null {
    const reader = new Reader(ptr, len);
    const equity = reader.readDecimal();
    const availableMargin = reader.readDecimal();
    const exposure = reader.readDecimal();
    const unrealizedPnl = reader.readDecimal();
    if (reader.failed) return null;
    return new AccountView(equity, availableMargin, exposure, unrealizedPnl);
  }
}

/**
 * Caps on the two count-prefixed lists in an [`AccountContext`] (ABI v3), matching the
 * host's `MAX_LEVERAGE_OVERRIDE_COUNT` (64) and `MAX_ALLOWED_INSTRUMENT_COUNT` (1024) in
 * `propify-sandbox-abi`. A list claiming more than its cap is malformed and decodes to
 * `null`, exactly as the host rejects an over-cap count before trusting it for bounds.
 */
export const MAX_LEVERAGE_OVERRIDE_COUNT: i32 = 64;
export const MAX_ALLOWED_INSTRUMENT_COUNT: i32 = 1024;

/**
 * The resolved drawdown rule inside an [`AccountContext`] (ABI v3): the kind discriminant
 * plus three host-computed `Decimal`s. `floor` is the authoritative current line the bot
 * should act on — for a [`DrawdownKind.Trailing`] account it already reflects the
 * high-water mark, for a [`DrawdownKind.Static`] account it is the fixed anchor minus the
 * limit. `kind` rides as a raw `u8` discriminant ([`DrawdownKind`]), mirroring how the
 * other enums ride as raw bytes on this boundary.
 */
export class DrawdownRule {
  constructor(
    public kind: u8,
    public limit: Decimal,
    public floor: Decimal,
    public highWaterMark: Decimal
  ) {}
}

/**
 * One per-asset-class leverage cap inside an [`AccountContext`]: the asset-class name
 * (a [`ByteSlice`] aliasing the host-written input buffer, no copy) and the cap. Mirrors
 * the Rust `(String, Decimal)` override pair.
 */
export class LeverageOverride {
  constructor(public assetClass: ByteSlice, public cap: Decimal) {}
}

/**
 * The read-only account context handed to a v3 bot each tick: the lifecycle status plus
 * the resolved rule set (daily-loss floor, drawdown kind and floor, leverage caps,
 * allowed instruments). The in-bot rules are for ADAPTATION, not trust — host-side risk
 * remains the sole backstop — so a well-behaved bot can size down near a floor or respect
 * a leverage cap, but a bot that ignores the context cannot exceed any limit.
 *
 * `status` is a raw [`AccountStatus`] discriminant. The asset-class names in
 * `leverageOverrides` and every symbol in `allowedInstruments` are [`ByteSlice`]s aliasing
 * the host-written input buffer, which the SDK keeps alive for the whole tick.
 */
export class AccountContext {
  constructor(
    public status: u8,
    public dailyLossLimit: Decimal,
    public dailyLossFloor: Decimal,
    public drawdown: DrawdownRule,
    public defaultLeverage: Decimal,
    public leverageOverrides: Array<LeverageOverride>,
    public allowedInstruments: Array<ByteSlice>,
    /**
     * The absolute equity level at which the evaluation challenge passes, or `null` on
     * a funded account (which has no profit target). Mirrors the Rust
     * `Option<Decimal>`: a `Some` value while Evaluation, `null` when Funded.
     */
    public profitTarget: Decimal | null
  ) {}

  /**
   * Decodes an account context from v3 wire bytes, mirroring the host codec's byte layout
   * EXACTLY: `status` (`u8`), `dailyLossLimit` and `dailyLossFloor` (`Decimal`), the inline
   * [`DrawdownRule`] (`kind` `u8` + three `Decimal`s), `defaultLeverage` (`Decimal`), a
   * `u32`-count-prefixed list of `(assetClass String, cap Decimal)` overrides, and a
   * `u32`-count-prefixed list of allowed-instrument `String`s.
   *
   * Returns `null` — so the driver skips the tick rather than act on garbage — on a
   * truncated buffer or an over-cap list count. An empty pair of lists decodes
   * successfully. Enum discriminants (`status`, `drawdown.kind`) ride as raw bytes: the
   * host is the trusted producer and always sends valid values, the same posture the
   * existing decoders take for the order enums and unchecked `Decimal` ranges.
   */
  static decode(ptr: usize, len: i32): AccountContext | null {
    const reader = new Reader(ptr, len);
    const status = reader.readU8();
    const dailyLossLimit = reader.readDecimal();
    const dailyLossFloor = reader.readDecimal();
    const drawdownKind = reader.readU8();
    const drawdownLimit = reader.readDecimal();
    const drawdownFloor = reader.readDecimal();
    const drawdownHwm = reader.readDecimal();
    const defaultLeverage = reader.readDecimal();
    if (reader.failed) return null;

    const overrideCount = reader.readU32();
    if (reader.failed) return null;
    // Reject an over-cap count exactly as the host does, before sizing the list.
    if (overrideCount > <u32>MAX_LEVERAGE_OVERRIDE_COUNT) return null;
    const leverageOverrides = new Array<LeverageOverride>(<i32>overrideCount);
    for (let i: u32 = 0; i < overrideCount; i++) {
      const assetClass = reader.readString();
      const cap = reader.readDecimal();
      if (reader.failed) return null;
      leverageOverrides[<i32>i] = new LeverageOverride(assetClass, cap);
    }

    const instrumentCount = reader.readU32();
    if (reader.failed) return null;
    if (instrumentCount > <u32>MAX_ALLOWED_INSTRUMENT_COUNT) return null;
    const allowedInstruments = new Array<ByteSlice>(<i32>instrumentCount);
    for (let i: u32 = 0; i < instrumentCount; i++) {
      const instrument = reader.readString();
      if (reader.failed) return null;
      allowedInstruments[<i32>i] = instrument;
    }

    // The `Option<Decimal>` profit-target tail: tag 0 = None (`null`, Funded), tag 1 =
    // Some + the decimal (Evaluation). Any other tag is malformed, exactly as the Rust
    // codec rejects an out-of-range Option tag.
    const profitTargetTag = reader.readU8();
    if (reader.failed) return null;
    let profitTarget: Decimal | null = null;
    if (profitTargetTag == 1) {
      profitTarget = reader.readDecimal();
      if (reader.failed) return null;
    } else if (profitTargetTag != 0) {
      return null;
    }

    const drawdown = new DrawdownRule(
      drawdownKind,
      drawdownLimit,
      drawdownFloor,
      drawdownHwm
    );
    return new AccountContext(
      status,
      dailyLossLimit,
      dailyLossFloor,
      drawdown,
      defaultLeverage,
      leverageOverrides,
      allowedInstruments,
      profitTarget
    );
  }
}

/**
 * The intent a bot emits: an order minus the two fields the guest may not set
 * (`intent_id`, which needs a clock the guest is denied, and the account, which the
 * guest must not name). The host stamps those when it lifts the body into a full
 * `OrderIntent`.
 *
 * `asset` is a [`ByteSlice`] so the bot can pass the snapshot's symbol straight
 * through. `price` and `triggerPrice` are `Decimal | null`, mirroring the Rust
 * `Option<Decimal>`.
 */
export class OrderIntentBody {
  constructor(
    public exchange: u8,
    public asset: ByteSlice,
    public productType: u8,
    public side: u8,
    public positionSide: u8,
    public orderType: u8,
    public timeInForce: u8,
    public quantity: Decimal,
    public price: Decimal | null,
    public triggerPrice: Decimal | null,
    public reduceOnly: bool
  ) {}
}
