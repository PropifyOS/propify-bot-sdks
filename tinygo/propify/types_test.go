// Off-target unit tests for the pure byte parser decodeMarketWindow (ABI v2).
//
// decodeMarketWindow calls no //go:wasmimport host function: it is a total byte
// parser over a []byte, so it runs under the SYSTEM `go` toolchain (go test ./...),
// not TinyGo (which is Docker-only here). These tests close the Phase 2 QA gap that
// no EXECUTED test asserted the decoded per-candle VALUES — only the empty window had
// flowed through the decoder via the sample. They are in `package propify` so they
// reach the unexported decodeMarketWindow, maxCandleCount, and the static
// candleStorage/tickWindow slots; that is test-only and changes no production source.
//
// The fixture bytes are hand-rolled little-endian, DELIBERATELY independent of the
// production Writer/PutDecimal codec, so a shared encoder bug cannot hide a decoder
// bug. The canonical wire layout mirrored here (and the host/Rust codec exactly):
//
//	MarketWindow = asset String (u32 LE len + UTF-8 bytes) + u32 LE candle count + count*Candle
//	Candle       = i64 LE timestamp_ms + 5 Decimal (open, high, low, close, volume)
//	Decimal      = i128 LE mantissa (16 bytes) + i32 LE scale (4 bytes) = 20 bytes
//	candle       = 8 + 5*20 = 108 bytes; oldest -> newest; cap MAX_CANDLE_COUNT = 256.
package propify

import "testing"

// --- hand-rolled little-endian encoders (independent of the codec under test) ---

func putU32(b *[]byte, v uint32) {
	*b = append(*b, byte(v), byte(v>>8), byte(v>>16), byte(v>>24))
}

func putI32(b *[]byte, v int32) {
	putU32(b, uint32(v))
}

func putI64(b *[]byte, v int64) {
	u := uint64(v)
	*b = append(*b,
		byte(u), byte(u>>8), byte(u>>16), byte(u>>24),
		byte(u>>32), byte(u>>40), byte(u>>48), byte(u>>56))
}

func putU64(b *[]byte, u uint64) {
	*b = append(*b,
		byte(u), byte(u>>8), byte(u>>16), byte(u>>24),
		byte(u>>32), byte(u>>40), byte(u>>48), byte(u>>56))
}

func putString(b *[]byte, s string) {
	putU32(b, uint32(len(s)))
	*b = append(*b, []byte(s)...)
}

// dec is a fixture Decimal as raw wire components: the two i128 mantissa halves and
// the scale. It is encoded low-half then high-half then scale, the canonical 20-byte
// Decimal layout.
type dec struct {
	low   uint64
	high  uint64
	scale int32
}

func putDecimal(b *[]byte, d dec) {
	putU64(b, d.low)
	putU64(b, d.high)
	putI32(b, d.scale)
}

// candleFixture is one candle's worth of fixture values: a timestamp plus the five
// OHLCV decimals in wire order.
type candleFixture struct {
	ts                          int64
	open, high, low, close, vol dec
}

func putCandle(b *[]byte, c candleFixture) {
	putI64(b, c.ts)
	putDecimal(b, c.open)
	putDecimal(b, c.high)
	putDecimal(b, c.low)
	putDecimal(b, c.close)
	putDecimal(b, c.vol)
}

// buildWindow assembles canonical MarketWindow bytes: asset String, u32 count, candles.
func buildWindow(asset string, candles []candleFixture) []byte {
	var b []byte
	putString(&b, asset)
	putU32(&b, uint32(len(candles)))
	for _, c := range candles {
		putCandle(&b, c)
	}
	return b
}

// assertDecimal fails the test if the decoded Decimal does not equal the fixture
// exactly (both mantissa halves and the scale).
func assertDecimal(t *testing.T, where string, got Decimal, want dec) {
	t.Helper()
	if got.MantissaLow != want.low || got.MantissaHigh != want.high || got.Scale != want.scale {
		t.Errorf("%s: got Decimal{low:%d high:%d scale:%d}, want {low:%d high:%d scale:%d}",
			where, got.MantissaLow, got.MantissaHigh, got.Scale, want.low, want.high, want.scale)
	}
}

// assertCandle fails the test if any decoded candle field diverges from the fixture.
func assertCandle(t *testing.T, where string, got Candle, want candleFixture) {
	t.Helper()
	if got.TimestampMs != want.ts {
		t.Errorf("%s: timestamp got %d, want %d", where, got.TimestampMs, want.ts)
	}
	assertDecimal(t, where+".open", got.Open, want.open)
	assertDecimal(t, where+".high", got.High, want.high)
	assertDecimal(t, where+".low", got.Low, want.low)
	assertDecimal(t, where+".close", got.Close, want.close)
	assertDecimal(t, where+".volume", got.Volume, want.vol)
}

// Three candles with distinct, non-trivial decimals across every field: the small
// positive 100.5 (mantissa 1005, scale 1), the tiny 0.002 (mantissa 2, scale 3), a
// NEGATIVE mantissa -12345 at scale 2 (high half all-ones two's complement), and a
// LARGE i128 mantissa whose high half is non-zero (1<<64 + 7 => low 7, high 1). Each
// candle shifts the values so no two candles or fields collide.
var threeCandles = []candleFixture{
	{
		ts:    1_699_999_880_000,
		open:  dec{low: 1005, high: 0, scale: 1},                        // 100.5
		high:  dec{low: 2, high: 0, scale: 3},                           // 0.002
		low:   dec{low: ^uint64(12345) + 1, high: ^uint64(0), scale: 2}, // -123.45 (-12345 i128)
		close: dec{low: 7, high: 1, scale: 0},                           // 1<<64 + 7
		vol:   dec{low: 999_999, high: 0, scale: 0},                     // 999999
	},
	{
		ts:    1_699_999_940_000,
		open:  dec{low: 5005, high: 0, scale: 2},                        // 50.05
		high:  dec{low: 9, high: 0, scale: 4},                           // 0.0009
		low:   dec{low: ^uint64(67890) + 1, high: ^uint64(0), scale: 1}, // -6789.0 (-67890 i128)
		close: dec{low: 42, high: 3, scale: 5},                          // (3<<64 + 42), scale 5
		vol:   dec{low: 1_234_567, high: 0, scale: 0},                   // 1234567
	},
	{
		ts:    1_700_000_000_000,
		open:  dec{low: 1, high: 0, scale: 28},                         // 1e-28 (max scale)
		high:  dec{low: 18_446_744_073_709_551_615, high: 0, scale: 0}, // u64 max in low half
		low:   dec{low: 0, high: ^uint64(0), scale: 0},                 // high half all-ones
		close: dec{low: 250_000, high: 0, scale: 2},                    // 2500.00
		vol:   dec{low: 0, high: 0, scale: 0},                          // exactly zero
	},
}

func TestDecodeMarketWindowPopulatedValues(t *testing.T) {
	asset := "BTC"
	buf := buildWindow(asset, threeCandles)

	win, ok := decodeMarketWindow(buf)
	if !ok {
		t.Fatalf("decodeMarketWindow returned ok == false for a valid populated window")
	}
	if string(win.Asset) != asset {
		t.Errorf("asset got %q, want %q", string(win.Asset), asset)
	}
	if len(win.Candles) != len(threeCandles) {
		t.Fatalf("candle count got %d, want %d", len(win.Candles), len(threeCandles))
	}
	for i, want := range threeCandles {
		assertCandle(t, candleLabel(i), win.Candles[i], want)
	}
}

func candleLabel(i int) string {
	return "candle[" + itoa(i) + "]"
}

// itoa avoids pulling strconv into a test that is otherwise dependency-free; the index
// is always a small non-negative int.
func itoa(i int) string {
	if i == 0 {
		return "0"
	}
	var digits []byte
	for i > 0 {
		digits = append([]byte{byte('0' + i%10)}, digits...)
		i /= 10
	}
	return string(digits)
}

func TestDecodeMarketWindowEmptyWarmup(t *testing.T) {
	// The warm-up window: a real asset but zero candles. It must decode successfully
	// (ok == true) with an empty candle slice — it is a valid window, not a failure.
	buf := buildWindow("ETH", nil)
	win, ok := decodeMarketWindow(buf)
	if !ok {
		t.Fatalf("empty (warm-up) window must decode with ok == true")
	}
	if string(win.Asset) != "ETH" {
		t.Errorf("asset got %q, want %q", string(win.Asset), "ETH")
	}
	if len(win.Candles) != 0 {
		t.Errorf("empty window candle count got %d, want 0", len(win.Candles))
	}
}

func TestDecodeMarketWindowFull256IntoStaticSlot(t *testing.T) {
	// A full window at exactly the cap (256). It must decode into the static
	// candleStorage [256]Candle slot with every candle present, first and last values
	// correct, and no overrun. Aliasing candleStorage proves the heap-free contract:
	// the decode wrote into the static slot rather than a fresh heap allocation.
	const n = maxCandleCount // 256
	candles := make([]candleFixture, n)
	for i := 0; i < n; i++ {
		v := uint64(i + 1)
		candles[i] = candleFixture{
			ts:    int64(1_700_000_000_000 + int64(i)),
			open:  dec{low: v, high: 0, scale: 1},
			high:  dec{low: v * 2, high: 0, scale: 2},
			low:   dec{low: ^v + 1, high: ^uint64(0), scale: 3}, // -v as i128
			close: dec{low: v, high: 1, scale: 4},               // high half set
			vol:   dec{low: v * 10, high: 0, scale: 0},
		}
	}
	buf := buildWindow("SOL", candles)

	win, ok := decodeMarketWindow(buf)
	if !ok {
		t.Fatalf("full 256-candle window must decode with ok == true")
	}
	if len(win.Candles) != n {
		t.Fatalf("full window candle count got %d, want %d", len(win.Candles), n)
	}
	// Aliases the static [256]Candle slot: proves no Go-heap allocation was used.
	if &win.Candles[0] != &candleStorage[0] {
		t.Errorf("decoded candles must alias the static candleStorage slot (heap-free contract)")
	}
	assertCandle(t, "candle[0]", win.Candles[0], candles[0])
	assertCandle(t, "candle[255]", win.Candles[n-1], candles[n-1])
}

func TestDecodeMarketWindowOverCapRejected(t *testing.T) {
	// A count of 257 exceeds MAX_CANDLE_COUNT (256). The decoder must reject it
	// (ok == false) before indexing the 256-slot array — exactly as the host does.
	var buf []byte
	putString(&buf, "BTC")
	putU32(&buf, maxCandleCount+1) // 257
	// No candle bytes needed: the over-cap check fires before any candle is read.
	if _, ok := decodeMarketWindow(buf); ok {
		t.Errorf("an over-cap count (257 > 256) must decode with ok == false")
	}
}

func TestDecodeMarketWindowTruncatedCandleRejected(t *testing.T) {
	// The header promises 2 candles but the buffer carries only one full candle plus a
	// few stray bytes. The bounded reader must fail (ok == false) rather than read past
	// the end.
	var buf []byte
	putString(&buf, "BTC")
	putU32(&buf, 2)
	putCandle(&buf, threeCandles[0]) // exactly one candle
	buf = append(buf, 0x01, 0x02, 0x03)
	if _, ok := decodeMarketWindow(buf); ok {
		t.Errorf("a window promising more candles than the buffer holds must decode with ok == false")
	}
}

func TestDecodeMarketWindowTruncatedHeaderRejected(t *testing.T) {
	// The asset length prefix claims 5 bytes but none follow: readString must fail and
	// the decode must report ok == false.
	buf := []byte{0x05, 0x00, 0x00, 0x00}
	if _, ok := decodeMarketWindow(buf); ok {
		t.Errorf("a truncated asset header must decode with ok == false")
	}
}

// buildParams assembles canonical StrategyParams bytes: a u32 pair count followed by
// each (name String, value Decimal) pair, mirroring the host/Rust codec.
func buildParams(names []string, values []dec) []byte {
	var b []byte
	putU32(&b, uint32(len(names)))
	for i := range names {
		putString(&b, names[i])
		putDecimal(&b, values[i])
	}
	return b
}

func TestStrategyParamsFindReturnsIndependentValues(t *testing.T) {
	// Two parameters with distinct Decimal values, including a negative two's-complement
	// mantissa so a collapsed result would be unmistakable. The old pointer-returning
	// Find aliased a single shared static slot, so a second lookup silently overwrote the
	// first; the value-returning API must keep each lookup fully independent.
	first := dec{low: 1005, high: 0, scale: 1}                         // 100.5
	second := dec{low: ^uint64(67890) + 1, high: ^uint64(0), scale: 2} // -678.90

	buf := buildParams([]string{"alpha", "beta"}, []dec{first, second})
	params := StrategyParams{buf: buf}

	// Look up the first parameter and keep the result, then look up the second. With the
	// old pointer API gotFirst would now read as the second value; by value it must not.
	gotFirst, okFirst := params.Find([]byte("alpha"))
	if !okFirst {
		t.Fatalf("Find(alpha) returned found == false for a present parameter")
	}
	gotSecond, okSecond := params.Find([]byte("beta"))
	if !okSecond {
		t.Fatalf("Find(beta) returned found == false for a present parameter")
	}

	// The exact case the old pointer API got wrong: the first result must be untouched
	// by the intervening second lookup.
	assertDecimal(t, "Find(alpha) after Find(beta)", gotFirst, first)
	assertDecimal(t, "Find(beta)", gotSecond, second)

	// An absent name returns the zero Decimal and found == false.
	if got, ok := params.Find([]byte("missing")); ok || got != (Decimal{}) {
		t.Errorf("Find(missing) = (%+v, %v), want (zero Decimal, false)", got, ok)
	}
}
