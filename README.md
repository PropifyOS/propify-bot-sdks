# PropifyOS bot SDKs

Guest SDKs for writing a trading bot that runs inside the PropifyOS sandbox, plus
the shared wire contract the host and every SDK agree on. One repository, three
languages, one ABI.

- `abi/` is the wire contract (the `propify-sandbox-abi` Rust crate). It defines the
  byte format and the boundary types both the host and the Rust SDK build on.
- `rust/` is the Rust guest SDK.
- `assemblyscript/` is the AssemblyScript guest SDK.
- `tinygo/` is the TinyGo guest SDK.
- `docs/` is the full getting-started guide for an external reader.

The example bots in this repository are reference code, not financial advice, and
are not tuned to anyone's risk.

## What a bot is

A bot is a deterministic WebAssembly module compiled for `wasm32`. The host loads
the module, calls it once per market candle, and tears it down. On each call the bot
reads its inputs and returns at most one order, or nothing.

The model in one paragraph: the host calls your bot once per candle. Your bot reads
the latest candle (and, under ABI v2, a bounded window of recent candles), the
strategy parameters set when the bot was deployed, and the account's own figures. It
returns `Some(order)` to act or `None` to wait. It has no clock beyond the candle
timestamp, no network, no filesystem, no randomness, and no memory that survives to
the next tick. The only way to know history is the window the host hands you. This is
what makes a bot's live behaviour reproducible in a backtest: identical inputs always
produce identical emitted bytes.

The host treats every guest as untrusted. It runs your module under a fuel budget, a
16 MiB linear-memory cap, a 256-candle window cap, and an empty linker that grants
only the `propify` capabilities below. Those limits are the boundary, not bugs. See
[docs/writing-a-bot.md](docs/writing-a-bot.md) for the full model.

## The ABI on one page

The ABI is the contract between the host and a guest. If you use one of the SDKs you
do not write any of this by hand; the SDK satisfies it. It is here so you can see the
whole surface at once.

### Versions

`ABI_VERSION` is `2`. A guest reports the version it targets through its
`abi_version()` export. The host supports v1 and v2 side by side: a v1 guest never
sees the candle window and runs exactly as before, a v2 guest may read it. All three
SDKs in this repository target v2 and expose the window read, and they keep the
single-snapshot read for simple bots.

### Required exports (the host checks these at load)

| Export        | Signature       | Notes                                                        |
|---------------|-----------------|--------------------------------------------------------------|
| `memory`      | linear memory   | The host reads and writes guest memory through this.         |
| `abi_version` | `() -> i32`     | Returns `2` for this SDK generation. The host calls it first.|
| `alloc`       | `(i32) -> i32`  | `(size) -> ptr`. Reserves a buffer; returns `0` on failure.  |
| `dealloc`     | `(i32, i32)`    | `(ptr, size)`. Releases a buffer from `alloc`.               |
| `on_tick`     | `() -> ()`      | The entry point, called once per tick.                       |

### Host capabilities (the only imports allowed)

Every capability lives in the `propify` import namespace. Any other import, including
any WASI import, causes the host to refuse the module at load time.

| Import                              | Signature               | Description                                        |
|-------------------------------------|-------------------------|----------------------------------------------------|
| `propify::host_read_market_data`    | `(ptr, len) -> i32`     | Reads the latest `MarketSnapshot` (v1 and v2).     |
| `propify::host_read_market_window`  | `(ptr, len) -> i32`     | Reads the `MarketWindow` of recent candles (v2).   |
| `propify::host_read_strategy_params`| `(ptr, len) -> i32`     | Reads the `StrategyParams`.                        |
| `propify::host_read_account_view`   | `(ptr, len) -> i32`     | Reads this account's `AccountView`.                |
| `propify::host_emit_intent`         | `(ptr, len) -> i32`     | Offers one encoded `OrderIntentBody` to the host.  |

### The inputs

- `MarketSnapshot`: the asset, `timestamp_ms`, and the latest OHLCV candle. All a
  simple bot needs.
- `MarketWindow` (v2): the asset and a bounded, time-ordered array of recent candles,
  oldest to newest, ending with the latest. Capped at 256 candles. During live
  warm-up, before that many candles exist, the window is shorter and may be empty; a
  window-aware bot must tolerate a short window.
- `StrategyParams`: a count-prefixed list of `(name, Decimal)` pairs, the deploy-time
  configuration. Look values up by name, not position.
- `AccountView`: `equity`, `available_margin`, `exposure`, and `unrealized_pnl` for
  this account only. No account id and no peer data ever cross the boundary.

### The output

`OrderIntentBody`: `exchange`, `asset`, `product_type`, `side`, `position_side`,
`order_type`, `time_in_force`, `quantity`, optional `price`, optional `trigger_price`,
and `reduce_only`. The body carries no order id and names no account; the host stamps
those when it lifts the body after the tick. Returning a body does not guarantee
placement: the host bounds-checks it and then runs the full platform risk gate.

### Exact money

All prices, quantities, and figures are exact fixed-point decimals, never `f64`. On
the wire a `Decimal` is 20 bytes: a 16-byte little-endian `i128` mantissa followed by
a 4-byte little-endian `i32` scale. Floating point is platform-dependent and would
break determinism, so no bot should use it for money.

### Wire format summary

All integers are little-endian. Booleans are `0` or `1`. Enums are a single byte
discriminant. Strings are a `u32` byte length followed by UTF-8 bytes. Optionals are
a tag byte (`0` absent, `1` present) then the value. A `Candle` is `timestamp_ms`
(`i64`) followed by the five OHLCV decimals. A `MarketWindow` is the asset string, a
`u32` candle count, then that many candles oldest to newest. Every top-level message
is capped at 64 KiB and must be fully consumed, so no guest can append hidden bytes.

The frozen discriminant table (do not renumber):

- `Exchange`: `Hyperliquid` = 0
- `ProductType`: `Spot` = 0, `Perp` = 1
- `OrderSide`: `Buy` = 0, `Sell` = 1
- `PositionSide`: `Long` = 0, `Short` = 1
- `OrderType`: `Market` = 0, `Limit` = 1, `StopMarket` = 2, `StopLimit` = 3, `TakeProfitMarket` = 4, `TakeProfitLimit` = 5
- `TimeInForce`: `Gtc` = 0, `Ioc` = 1, `Fok` = 2, `Gtx` = 3

## Pick an SDK and build it

Each SDK directory has its own README with prerequisites and details.

### Rust

```bash
cd rust
cargo test
cargo build --example minimal_bot --target wasm32-unknown-unknown --release
```

The build emits a guest module at
`target/wasm32-unknown-unknown/release/examples/minimal_bot.wasm`. See
[rust/README.md](rust/README.md).

### AssemblyScript

```bash
cd assemblyscript
pnpm install --frozen-lockfile
pnpm build
pnpm test
```

The build emits `build/sample.wasm`. See
[assemblyscript/README.md](assemblyscript/README.md).

### TinyGo

```bash
cd tinygo
go test ./...
./build-repro.sh
```

The build emits `build/sample.wasm` and needs TinyGo 0.41.1. See
[tinygo/README.md](tinygo/README.md).

## Documentation

- [docs/writing-a-bot.md](docs/writing-a-bot.md): the full guide. The execution
  model, the data your bot receives and returns, the exact decimal format, both the
  v1 snapshot and the v2 window, and per-language build instructions.

## License

Apache-2.0. See [LICENSE](LICENSE).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md).
