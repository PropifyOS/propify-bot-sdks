# Writing a bot for the PropifyOS sandbox

**Audience:** bot creators. You need familiarity with at least one of the three
supported languages (Rust, AssemblyScript, TinyGo) but no prior knowledge of
WebAssembly internals. The SDK handles the boundary.

This guide covers the complete creator API: the execution model, the data your bot
receives and returns, the exact decimal money format, the ABI contract for advanced
readers, and the per-language build instructions.

---

## What a bot is

A bot is a WebAssembly module compiled for `wasm32`. The PropifyOS sandbox host
(backed by Wasmtime) loads the module, calls it once per market tick, and tears it
down. The module receives a read-only snapshot of the latest market candle, a bounded
window of recent candles, the strategy's configuration parameters, the account's
current figures, and a read-only account context with the resolved rule set. It then
returns at most one order intent or nothing.

---

## The execution model

### Zero ambient authority

The host grants the guest a small set of read or emit capabilities and nothing else.
It denies every WebAssembly proposal or import that could give a guest hidden
influence over the environment: threads, relaxed SIMD, WASI, and any import not on
the `propify` allow-list.

Your bot has no access to:

- The system clock. The only time the guest ever sees is `MarketSnapshot.timestamp_ms`
  (and the per-candle timestamps in the window), which the host injects.
- The network.
- The filesystem.
- Environment variables.
- Randomness.

If your code reads the wall clock, opens a file, opens a socket, or calls a
random-number function, the host refuses the module at load time (if the import is
present) or traps at runtime. There is no workaround: the host denies these
capabilities at the engine level.

### Fresh instance per tick

The host creates a new Wasmtime store and guest instance for every tick. Linear
memory is zeroed at instantiation. Nothing persists from one tick to the next. Do not
try to carry state across ticks through global variables or module-level statics; they
reset each time. If you need history, read the window: it is the host-supplied way to
see past candles without keeping state.

### Determinism

Because the guest has no clock and no randomness, identical inputs must produce
identical emitted bytes. This is not a soft expectation; it is the property that makes
a live bot's behaviour reproducible in a backtest. The window and the account context
do not change this: both are produced and supplied by the host, so identical tick
inputs always yield identical host-supplied bytes. Write your decision logic as a pure
function of the inputs you receive.

---

## ABI version: v3

`ABI_VERSION` is `3`. Your guest reports the version it targets through its
`abi_version()` export, which the SDK sets for you. The host accepts only guests that
report `3`; the prior v1 and v2 support is dropped.

v3 defines the complete input surface: the single-candle snapshot, the multi-candle
window, the strategy parameters, the account view, and the read-only account context.
The window and the account context are part of the contract; a simple bot ignores them
without any version branching. Every v3 bot also embeds a manifest inside the artifact;
see "Bot manifest" in "The data your bot receives" below.

Reading the window or the account context is optional for individual bots: a
snapshot-only bot simply ignores both.

---

## The data your bot receives

The host encodes the inputs before calling `on_tick` and serves them through the host
read capabilities.

### MarketSnapshot

A single market observation: the latest candle for one asset.

| Field          | Type      | Description                                                                 |
|----------------|-----------|-----------------------------------------------------------------------------|
| `asset`        | string    | Asset symbol, for example `"BTC"`.                                          |
| `timestamp_ms` | `i64`     | Observation time, milliseconds since the Unix epoch. The only clock you see.|
| `open`         | `Decimal` | Open price for the candle.                                                  |
| `high`         | `Decimal` | High price.                                                                 |
| `low`          | `Decimal` | Low price.                                                                  |
| `close`        | `Decimal` | Close price.                                                                |
| `volume`       | `Decimal` | Traded volume over the candle.                                             |

### MarketWindow

The asset and a bounded, time-ordered array of recent candles, oldest to newest,
ending with the latest. Each candle carries `timestamp_ms` and the same five OHLCV
decimals as the snapshot (the asset is carried once on the window, not per candle).

The window is capped at 256 candles. During live warm-up, before that many candles
exist, the window is naturally shorter and may be empty. A short or empty window is a
valid window, not a decode failure: a window-aware bot must define what it does before
it has enough history (for example, emit nothing until the slow average has enough
candles). The SDKs hand the window through unchanged and do not special-case warm-up
for you.

### StrategyParams

A count-prefixed, ordered list of named decimal values. The host passes these through
without interpreting them; they are the creator's tuning knobs, set when the strategy
is deployed.

Look values up by name, not by position. The order may differ across deployments. In
Rust, `params.params` is a `Vec<(String, Decimal)>`; iterate and find by name. In
AssemblyScript, call `params.find(NAME)` where `NAME` is a `StaticArray<u8>` of ASCII
bytes. In TinyGo, call `params.Find(name)` where `name` is a `[]byte` of ASCII bytes.
All three SDKs compare names at the byte level rather than decoding UTF-8, which is
faster and avoids allocation.

### AccountView

This account's own figures only. There is deliberately no account id and no data about
any other account.

| Field              | Type      | Description                                    |
|--------------------|-----------|------------------------------------------------|
| `equity`           | `Decimal` | Account equity.                                |
| `available_margin` | `Decimal` | Margin available to open new exposure.         |
| `exposure`         | `Decimal` | Current open exposure.                         |
| `unrealized_pnl`   | `Decimal` | Unrealized profit and loss on open positions.  |

`exposure` is the lever that lets a stateless bot know where it already stands, so it
can emit the difference toward a target rather than tracking its own past orders.

### AccountContext

The read-only account rule set and lifecycle status for this tick.

| Field                 | Type                         | Description                                                                                              |
|-----------------------|------------------------------|----------------------------------------------------------------------------------------------------------|
| `status`              | `AccountStatus`              | `Evaluation` (0) or `Funded` (1).                                                                        |
| `daily_loss_limit`    | `Decimal`                    | Fixed daily loss allowance, in account currency.                                                         |
| `daily_loss_floor`    | `Decimal`                    | Host-computed equity level at which today's loss limit is breached.                                      |
| `drawdown`            | `DrawdownRule`               | The resolved drawdown rule (see below).                                                                  |
| `default_leverage`    | `Decimal`                    | Default maximum leverage.                                                                                |
| `leverage_overrides`  | list of (asset_class, cap)   | Per-asset-class leverage caps, for example BTC 5x. Count-prefixed list.                                  |
| `allowed_instruments` | list of string               | Permitted asset symbols. Count-prefixed list.                                                            |
| `profit_target`       | optional `Decimal`           | Absolute equity level the account must reach to pass an evaluation. `None`/`null` when the account is Funded (a funded account carries no profit target). |

`DrawdownRule` fields:

| Field            | Type           | Description                                                                                                           |
|------------------|----------------|-----------------------------------------------------------------------------------------------------------------------|
| `kind`           | `DrawdownKind` | `Static` (0) for 1-step tiers; `Trailing` (1) for 2-step tiers.                                                      |
| `limit`          | `Decimal`      | Drawdown allowance.                                                                                                   |
| `floor`          | `Decimal`      | Host-computed current drawdown floor. For a trailing account this already reflects the high-water mark.               |
| `high_water_mark`| `Decimal`      | Host-computed high-water mark. Use `floor` for risk decisions; `high_water_mark` is for display or logging.           |

The in-bot rules are for adaptation only. The host risk gate enforces every limit
regardless of what the bot reads or emits; a bot that ignores the context cannot exceed
any limit.

### Bot manifest

Every v3 bot embeds a manifest as a `propify_manifest` WebAssembly custom section
inside the artifact. The host extracts and validates it at submission; the manifest
fields are hashed as part of `ArtifactId` (sha256 of the module bytes), so the
manifest cannot be changed without changing the content address.

The manifest carries: `name`, `description`, `version` (semver), `license` (SPDX
identifier), optional `image_sha256` (content hash of a separately uploaded 512x512
PNG), `author_name`, `author_email`, `author_erc20` (EIP-55 checksummed address), and
`source_repo_url` (an `https` URL).

How the section is emitted differs per language. In Rust, the toolchain emits it
natively: add a `build.rs` that builds a `BotManifest`, calls `.encode()`, and writes
the bytes to `$OUT_DIR/propify_manifest.bin`, then call `declare_manifest!()` in the
bot alongside `register_bot!`. In AssemblyScript and TinyGo there is no portable
link-section attribute, so the section is injected after the build with the
`manifest-encoder` tool; see `tools/manifest-encoder/README.md`.

---

## What the bot returns

The bot returns at most one order intent per tick, or nothing.

- Rust: `Option<OrderIntentBody>` (`Some(body)` or `None`).
- AssemblyScript: `OrderIntentBody | null`.
- TinyGo: `*OrderIntentBody` (`nil` for nothing).

Returning a body does not guarantee placement. The host runs a boundary check
(quantity positive, prices non-negative) and then the full platform risk gate before
any order reaches the venue. A bot cannot bypass risk limits.

### OrderIntentBody

| Field           | Type               | Description                                                                                  |
|-----------------|--------------------|----------------------------------------------------------------------------------------------|
| `exchange`      | `Exchange`         | Target venue. Only `Hyperliquid` is valid.                                                   |
| `asset`         | string             | Asset symbol.                                                                                |
| `product_type`  | `ProductType`      | `Spot` or `Perp`.                                                                            |
| `side`          | `OrderSide`        | `Buy` or `Sell`.                                                                             |
| `position_side` | `PositionSide`     | `Long` or `Short`.                                                                           |
| `order_type`    | `OrderType`        | `Market`, `Limit`, `StopMarket`, `StopLimit`, `TakeProfitMarket`, or `TakeProfitLimit`.     |
| `time_in_force` | `TimeInForce`      | `Gtc`, `Ioc`, `Fok`, or `Gtx`.                                                              |
| `quantity`      | `Decimal`          | Order size. Must be strictly positive.                                                      |
| `price`         | optional `Decimal` | Limit price. Required for `Limit` and `StopLimit`; omit for market orders. Not negative.    |
| `trigger_price` | optional `Decimal` | Trigger price. Required for stop and take-profit types. Not negative when present.          |
| `reduce_only`   | `bool`             | When `true`, the order may only reduce an existing position.                                |

The body carries no order id and names no account. The guest cannot mint an id (it has
no clock) and must not choose which account it trades for. The host stamps both when it
lifts the body into a full order after the tick returns.

---

## Exact decimal money

All prices, quantities, equity figures, and strategy parameter values are exact
fixed-point decimals. The wire format carries each one as 20 bytes: a 16-byte
little-endian two's-complement `i128` mantissa followed by a 4-byte little-endian
`i32` scale.

The host never uses `f64` or `f32` for money, and neither should your bot.
Floating-point arithmetic is platform-dependent; a bot that uses floats for prices or
quantities can produce different bytes on different hardware or different compiler
versions, breaking determinism and causing live behaviour to diverge from the
backtest.

Each SDK exposes `Decimal` in a way that fits the language.

- **Rust:** `rust_decimal::Decimal`. Construct small values with
  `Decimal::new(mantissa, scale)` or the `dec!(...)` macro. `Decimal::new(1, 3)` is
  exactly `0.001`.
- **AssemblyScript:** a `Decimal` class with `mantissaLow: i64`, `mantissaHigh: i64`,
  and `scale: i32`. AssemblyScript has no native `i128`, so the 128-bit mantissa is
  split into two halves in little-endian order. For values that fit in a single `i64`,
  use `Decimal.fromI64(mantissa, scale)`, which sign-extends the low half.
  `Decimal.fromI64(1, 3)` is exactly `0.001`.
- **TinyGo:** a `Decimal` struct with `MantissaLow uint64`, `MantissaHigh uint64`, and
  `Scale int32`, for the same reason. Use `DecimalFromI64(mantissa, scale)` for values
  that fit in a single `int64`. `DecimalFromI64(1, 3)` is exactly `0.001`.

---

## The ABI contract

If you use one of the three SDKs as shown below, the SDK satisfies every contract
requirement. This section documents the boundary for creators who want to understand
what the host checks, write an SDK in another language, or audit the guest's surface.

### Required exports

The host validates these exports at load time and refuses any module that is missing
one, has the wrong type, or has the wrong signature.

| Export        | Kind     | Signature      | Notes                                                                              |
|---------------|----------|----------------|------------------------------------------------------------------------------------|
| `memory`      | memory   | linear memory  | Auto-exported by `cdylib` Rust crates and by the `wasm-unknown` TinyGo target.     |
| `abi_version` | function | `() -> i32`    | Returns `3`. The host calls it before any tick and refuses a guest that returns any other value. |
| `alloc`       | function | `(i32) -> i32` | `(size) -> ptr`. Reserves a buffer the host writes inputs into. `0` means failure. |
| `dealloc`     | function | `(i32, i32)`   | `(ptr, size)`. Releases a buffer from `alloc`.                                      |
| `on_tick`     | function | `() -> ()`     | The entry point, called once per tick after version negotiation.                   |

### Required host imports

The host grants these capabilities, all in the `propify` import namespace. Any other
import, including any WASI import, causes the host to refuse the module at load time.

| Import                                   | Signature             | Description                                              |
|------------------------------------------|-----------------------|----------------------------------------------------------|
| `propify::host_read_market_data`         | `(ptr, len) -> i32`   | Reads the encoded latest `MarketSnapshot`.               |
| `propify::host_read_market_window`       | `(ptr, len) -> i32`   | Reads the encoded `MarketWindow`.                        |
| `propify::host_read_strategy_params`     | `(ptr, len) -> i32`   | Reads the encoded `StrategyParams`.                      |
| `propify::host_read_account_view`        | `(ptr, len) -> i32`   | Reads the encoded `AccountView`.                         |
| `propify::host_read_account_context`     | `(ptr, len) -> i32`   | Reads the encoded `AccountContext` for this tick (v3).   |
| `propify::host_emit_intent`              | `(ptr, len) -> i32`   | Offers the encoded `OrderIntentBody` to the host.        |

A snapshot-only bot never calls `host_read_market_window` or
`host_read_account_context`. The SDKs read both for you when the bot is window- or
context-aware.

### Read protocol

The read functions share one protocol. The guest allocates a buffer of some initial
capacity and calls the function with `(ptr, capacity)`. The host encodes the message
and returns its full byte length `n`. If `n <= capacity`, the host wrote the bytes and
the guest decodes them. If `n > capacity`, the host wrote nothing and returned `n`; the
guest frees the buffer, allocates exactly `n` bytes, and calls once more. A negative
return (`-1`) means an internal host error; the SDK treats this as no input this tick
and returns without emitting.

The SDKs start with a 256-byte buffer. For a typical snapshot, parameter list, or
account view this is large enough, so the common case is one host call with no retry. A
full window is larger and may take the single retry.

### Emit protocol

To emit, the guest encodes an `OrderIntentBody` into its own linear memory and calls
`host_emit_intent(ptr, len)`. The host reads those bytes, decodes them, runs a bounds
check, and on success records the intent. Return values: `0` accepted, `-2` the codec
rejected the bytes, `-3` the bounds check rejected the intent (`-1` is a short or
out-of-bounds guest read). The SDK ignores the return after emitting; the host has
already decided and the guest cannot retry. The host records only the first accepted
emission per tick; later emissions are ignored.

### Wire codec summary

All integers are little-endian. Booleans are `0` or `1`. Enums are a single byte
discriminant. Strings are a `u32` byte length followed by UTF-8 bytes. A `Decimal` is
20 bytes: a 16-byte `i128` mantissa and a 4-byte `i32` scale. Optionals are a tag byte
(`0` absent, `1` present) then the value. A `Candle` is `timestamp_ms` (`i64`) then the
five OHLCV decimals. A `MarketWindow` is the asset string, a `u32` candle count, then
that many candles oldest to newest, capped at 256. Every top-level message is capped at
64 KiB and must be fully consumed; trailing bytes are rejected.

The frozen discriminant table (do not renumber):

- `Exchange`: `Hyperliquid` = 0
- `ProductType`: `Spot` = 0, `Perp` = 1
- `OrderSide`: `Buy` = 0, `Sell` = 1
- `PositionSide`: `Long` = 0, `Short` = 1
- `OrderType`: `Market` = 0, `Limit` = 1, `StopMarket` = 2, `StopLimit` = 3, `TakeProfitMarket` = 4, `TakeProfitLimit` = 5
- `TimeInForce`: `Gtc` = 0, `Ioc` = 1, `Fok` = 2, `Gtx` = 3

---

## Rust SDK

### Prerequisites

- Rust 1.95.0 or later.
- The `wasm32-unknown-unknown` target:

```bash
rustup target add wasm32-unknown-unknown
```

### A minimal bot

Create a crate with `crate-type = ["cdylib"]`, add `propify-bot-sdk` as a dependency,
implement the `Bot` trait, and register it with `register_bot!`:

```rust
use propify_bot_sdk::{
    AccountContext, AccountView, Bot, Exchange, MarketSnapshot, MarketWindow,
    OrderIntentBody, OrderSide, OrderType, PositionSide, ProductType, StrategyParams,
    TimeInForce, declare_manifest, register_bot,
};
use rust_decimal::Decimal;

struct MyBot;

impl Bot for MyBot {
    fn on_tick(
        &mut self,
        market: &MarketSnapshot,
        _window: &MarketWindow,
        params: &StrategyParams,
        _account: &AccountView,
        _context: &AccountContext,
    ) -> Option<OrderIntentBody> {
        let quantity = params
            .params
            .iter()
            .find(|(name, _)| name == "quantity")
            .map(|(_, value)| *value)
            .unwrap_or_else(|| Decimal::new(1, 3)); // 0.001

        Some(OrderIntentBody {
            exchange: Exchange::Hyperliquid,
            asset: market.asset.clone(),
            product_type: ProductType::Perp,
            side: OrderSide::Buy,
            position_side: PositionSide::Long,
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            quantity,
            price: None,
            trigger_price: None,
            reduce_only: false,
        })
    }
}

register_bot!(MyBot);
declare_manifest!();
```

`register_bot!` accepts a unit struct name (`register_bot!(MyBot)`) or a constructor
expression (`register_bot!(MyBot::new())`). It generates the required exports only when
compiling for `target_arch = "wasm32"`. Building for the host target (for example under
`cargo test`) produces no FFI exports, so your decision logic stays unit-testable
off-target. Because the host re-instantiates the module per tick, the macro constructs
a fresh bot for every `on_tick` call; there is no cross-tick state.

`declare_manifest!()` embeds the `propify_manifest` custom section into the artifact.
It reads the encoded bytes from `$OUT_DIR/propify_manifest.bin`, which a small
`build.rs` in the bot crate writes. Add `propify-sandbox-abi` under
`[build-dependencies]` and copy the `build.rs` from `rust/build.rs` as your template.

This repository ships this bot as a runnable example at
[`rust/examples/minimal_bot.rs`](../rust/examples/minimal_bot.rs).

### Building

```bash
cd rust
cargo build --example minimal_bot --target wasm32-unknown-unknown --release
```

The guest module is written to
`target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm`. For a publishable,
byte-reproducible artifact, build with the `release` profile, pass path-remapping flags
through `CARGO_ENCODED_RUSTFLAGS` so the output does not depend on where your checkout
lives, and normalize the output with `wasm-tools strip -d producers`. See
[rust/README.md](../rust/README.md) for the reproducible-build recipe.

---

## AssemblyScript SDK

### Prerequisites

- Node.js 24.
- pnpm 11.9.0.
- AssemblyScript 0.28.19 (installed from the lockfile, no global install needed).

### Structure

Your bot source file goes in `assembly/`. The SDK files (`assembly/index.ts`,
`assembly/types.ts`, `assembly/decimal.ts`, `assembly/wire.ts`, `assembly/imports.ts`,
`assembly/noop.ts`) are already present; you add your bot file and point the `asc`
entry at it. The shipped example is `assembly/sample.ts`.

### A minimal bot

Subclass `Bot` and implement `onTick`. AssemblyScript has no macro system, so you also
write the ABI export functions by hand. This is a small, one-time piece of wiring:

```typescript
import "./noop"; // pulls in the no-op abort override

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

// ASCII bytes for "quantity": q,u,a,n,t,i,t,y
const QUANTITY_NAME: StaticArray<u8> = [113, 117, 97, 110, 116, 105, 116, 121];

class MyBot extends Bot {
  onTick(
    market: MarketSnapshot,
    window: MarketWindow,
    params: StrategyParams,
    account: AccountView,
    context: AccountContext
  ): OrderIntentBody | null {
    const found = params.find(QUANTITY_NAME);
    const quantity = found !== null ? found : Decimal.fromI64(1, 3); // 0.001

    return new OrderIntentBody(
      Exchange.Hyperliquid,
      market.asset,
      ProductType.Perp,
      OrderSide.Buy,
      PositionSide.Long,
      OrderType.Market,
      TimeInForce.Ioc,
      quantity,
      null, // price
      null, // trigger_price
      false // reduce_only
    );
  }
}

// ABI exports (the hand-written equivalent of register_bot!)

export function abi_version(): i32 { return sdkAbiVersion(); }
export function alloc(size: i32): i32 { return sdkAlloc(size); }
export function dealloc(ptr: i32, size: i32): void { sdkDealloc(ptr, size); }
export function on_tick(): void { runTick(new MyBot()); }
```

The `import "./noop"` line is required: without it, `asc` cannot resolve the
`--use abort=assembly/noop/propifyAbort` override and the build fails. The `noop`
module has no runtime effect; it only provides the override target. The `memory` export
is emitted automatically because the build does not pass `--importMemory`.

### Building

```bash
cd assemblyscript
pnpm install --frozen-lockfile
pnpm build
```

`pnpm build` runs `build-repro.sh`, the single source of truth for the compiler flags.
Two flags are required for correctness, not just size:

- `--runtime stub`: uses the bump-allocator runtime. The default GC runtime imports
  `env.abort`, which is not on the allow-list and causes the host to refuse the module.
- `--use abort=assembly/noop/propifyAbort`: replaces the default `abort` with a no-op,
  so a runtime assertion cannot try to call into the environment.

The output artifact is `build/sample.wasm`. Run the SDK tests with `pnpm test`.

---

## TinyGo SDK

### Prerequisites

- Go 1.24 or later (to run the host-side tests).
- TinyGo 0.41.1 with binaryen (for the `-opt=z` pass). See the
  [TinyGo installation guide](https://tinygo.org/getting-started/install/), or build
  through the official `tinygo/tinygo:0.41.1` container.

### Structure

The SDK lives at `tinygo/propify/`. The module path is
`github.com/PropifyOS/propify-bot-sdks/tinygo`. Your bot is a `package main` program
under `tinygo/` (or a sub-directory); import the `propify` package from that module
path. The shipped example is `tinygo/sample/main.go`.

### A minimal bot

Implement the `Bot` interface (one method, `OnTick`) and wire the ABI exports. Go has
no macros, so the wiring is a handful of functions you write once:

```go
package main

import "github.com/PropifyOS/propify-bot-sdks/tinygo/propify"

// ASCII bytes for "quantity": q,u,a,n,t,i,t,y
var quantityName = []byte{113, 117, 97, 110, 116, 105, 116, 121}

type MyBot struct{}

// orderBody is a package-level static slot for the returned body. Do not return the
// address of a local; see the heap note below.
var orderBody propify.OrderIntentBody

func (MyBot) OnTick(
    market *propify.MarketSnapshot,
    window *propify.MarketWindow,
    params *propify.StrategyParams,
    account *propify.AccountView,
    context *propify.AccountContext,
) *propify.OrderIntentBody {
    quantity, ok := params.Find(quantityName)
    if !ok {
        quantity = propify.DecimalFromI64(1, 3) // 0.001
    }

    orderBody = propify.OrderIntentBody{
        Exchange:     propify.ExchangeHyperliquid,
        Asset:        market.Asset,
        ProductType:  propify.ProductTypePerp,
        Side:         propify.OrderSideBuy,
        PositionSide: propify.PositionSideLong,
        OrderType:    propify.OrderTypeMarket,
        TimeInForce:  propify.TimeInForceIoc,
        Quantity:     quantity,
        Price:        nil,
        TriggerPrice: nil,
        ReduceOnly:   false,
    }
    return &orderBody
}

// ABI exports (the hand-written equivalent of register_bot!).
// Use //export, not //go:wasmexport. See the note below.

var myBot MyBot

//export abi_version
func abiVersion() int32 { return propify.AbiVersion() }

//export alloc
func alloc(size int32) int32 { return propify.Alloc(size) }

//export dealloc
func dealloc(ptr int32, size int32) { propify.Dealloc(ptr, size) }

//export on_tick
func onTick() { propify.RunTick(&myBot) }

func main() {} // required for package main; never called by the wasm-unknown target
```

### `//export` vs `//go:wasmexport`

Always use the legacy `//export name` directive. Do not use `//go:wasmexport name`. The
newer directive implements the Go reactor lifecycle: it wraps every export in a guard
that traps unless `_initialize` (or `_start`) has run first. The host does not call
`_initialize`; its protocol is `abi_version` then `on_tick`, with `alloc` and `dealloc`
present for signature checking. Under `//go:wasmexport`, even `abi_version` would trap
before any of your code runs. `//export` emits a plain export with no such guard, which
is what a host that does not drive the reactor lifecycle needs.

### Heap allocations

Because the host never calls `_initialize`, the Go allocator is never set up, so any Go
heap allocation (a `&Struct{}` that escapes, `make([]byte, n)`, or a growing `append`)
reads an uninitialised allocator and traps. The SDK works around this with a static
bump arena (a package-level array in `propify/memory.go`) that lives in the module's
data segment, which WebAssembly zeroes at instantiation with no init code.

The rules for your bot code:

- Store your returned `OrderIntentBody` in a package-level variable and return its
  address, as shown. Do not return the address of a local.
- Store your `Bot` implementation in a package-level variable and pass its address to
  `RunTick`.
- Do not call `make`, `new`, or a growing `append` from any export reachable without
  `_initialize`.

The `propify.RunTick`, `propify.Alloc`, `propify.Find`, and the wire decoder functions
all operate on arena-backed memory and follow these rules internally.

### Building

```bash
cd tinygo
go test ./...
./build-repro.sh
```

`go test ./...` runs the SDK's host-side decoder tests. `build-repro.sh` compiles the
sample to `build/sample.wasm` with TinyGo 0.41.1 and normalizes it with
`wasm-tools strip -d producers`. The flags it passes to `tinygo build` are
`-target=wasm-unknown` (the `wasm32` target without WASI), `-panic=trap`, `-no-debug`,
`-opt=z`, and `-scheduler=none`. To build through the container instead of a local
TinyGo:

```bash
docker run --rm -v "$PWD":/src -w /src tinygo/tinygo:0.41.1 \
  tinygo build -target=wasm-unknown -no-debug -opt=z -panic=trap -scheduler=none \
  -o /src/build/sample.wasm /src/sample
```

---

## Limits and rules

The host treats every guest as untrusted. These limits are the boundary, not bugs.

### Module file size

The host refuses any uploaded `.wasm` larger than its upload cap. A typical
release-optimised strategy is well under this. The limit bounds compile-time work
at upload.

### Import allow-list

The host validates every import at load time. The only permitted imports are the
`propify` capabilities listed above. Any other import, including any WASI function, any
`env.*` function, or any import in another namespace, causes the host to refuse the
module. The SDKs produce exactly the permitted imports and nothing else.

### Linear memory cap

A guest's linear memory may not grow past 16 MiB. A `memory.grow` that would exceed
this traps the guest rather than letting it proceed with a failed grow. Wasmtime's
store limiter enforces this, not the module.

### CPU budget (fuel)

The host meters guest execution with Wasmtime fuel, a deterministic instruction
counter. A bot that exceeds its per-tick budget is trapped, and an infinite loop is
always terminated. Because fuel is deterministic, it adds no non-determinism to the
guest's output.

### The window cap and warm-up

The market window is capped at 256 candles. Before that many candles exist (live
warm-up), the window is shorter and may be empty. Define what your bot does on a short
window; the host serves what it has.

### One intent per tick

The host records only the first accepted emission from a tick. A second emission in the
same tick is ignored. Return one body or nothing.

### Risk gate

Every intent the host records still passes through the platform pre-order risk gate
before it reaches the venue. The gate enforces the funded account's exposure limits,
drawdown rules, and any other constraints set by the prop firm. A bot cannot override
or bypass these checks. An intent that passes the bounds check but fails the risk gate
is silently discarded.

---

## Reproducible builds

Every uploaded `.wasm` is identified by its `ArtifactId`, the SHA-256 of the module's
bytes. The per-SDK `build-repro.sh` scripts produce a normalized, byte-identical
`.wasm` from the same source, so a consumer or auditor can rebuild independently and
confirm the hash matches the published artifact. Each SDK README documents its
reproducible recipe and pinned toolchain.
