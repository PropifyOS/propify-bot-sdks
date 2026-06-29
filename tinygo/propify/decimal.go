// Exact fixed-point money, carried the way the wire carries it.
//
// The wire `Decimal` is a 16-byte little-endian i128 mantissa followed by a 4-byte
// little-endian i32 scale (20 bytes total), the same shape `rust_decimal::Decimal`
// serializes to in `propify-sandbox-abi`. Go has no native int128, so the mantissa is
// carried as two 64-bit halves: `MantissaLow` (the low 8 bytes) and `MantissaHigh`
// (the high 8 bytes, sign-extended). Writing low-half-then-high-half in little-endian
// reproduces the exact 16-byte i128 layout, and reading does the inverse.
//
// We only ever carry and round-trip the raw mantissa bytes — no 128-bit arithmetic is
// performed — so two 64-bit halves are sufficient and exact. Determinism rule: this is
// the only numeric representation of money on the boundary; there is no `float64`
// anywhere near it. The halves are `uint64` so the little-endian byte shifts in the
// codec are plain logical shifts with no sign-bit surprises.
package propify

// Decimal mirrors the AssemblyScript `Decimal` and the Rust `(i128 mantissa, i32
// scale)` pair. It is a value type: copying it copies the three integer fields, with
// no aliasing and no allocation.
type Decimal struct {
	// MantissaLow is the low 8 bytes of the i128 mantissa (little-endian).
	MantissaLow uint64
	// MantissaHigh is the high 8 bytes of the i128 mantissa (little-endian,
	// sign-extended).
	MantissaHigh uint64
	// Scale is the number of fractional digits; the valid range is 0..=28.
	Scale int32
}

// DecimalFromI64 builds a Decimal whose mantissa fits in a single int64 (the common
// case for a small, hand-written default such as 0.001). The high half is the sign
// extension of the low half so the two-half form still encodes the exact i128: a
// negative mantissa needs all-ones in the high 8 bytes to be the correct two's
// complement.
func DecimalFromI64(mantissa int64, scale int32) Decimal {
	// Arithmetic right shift by 63 yields 0 for a non-negative mantissa and all-ones
	// (-1) for a negative one; reinterpreting that as uint64 gives the high half.
	high := uint64(mantissa >> 63)
	return Decimal{MantissaLow: uint64(mantissa), MantissaHigh: high, Scale: scale}
}
