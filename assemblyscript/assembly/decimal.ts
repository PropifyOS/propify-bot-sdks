// Exact fixed-point money, carried the way the wire carries it.
//
// The wire `Decimal` is a 16-byte little-endian i128 mantissa followed by a 4-byte
// little-endian i32 scale (20 bytes total), the same shape `rust_decimal::Decimal`
// serializes to in `propify-sandbox-abi`. AssemblyScript has no native i128, so the
// mantissa is carried as two i64 halves: `mantissaLow` (the low 8 bytes) and
// `mantissaHigh` (the high 8 bytes). Because wasm memory is little-endian and
// `load`/`store` are little-endian, writing low-half-then-high-half reproduces the
// exact 16-byte i128 layout, and reading does the inverse.
//
// We only ever carry and round-trip the raw mantissa bytes — no 128-bit arithmetic
// is performed — so two i64 halves are sufficient and exact. Determinism rule: this
// is the only numeric representation of money on the boundary; there is no `f64`
// anywhere near it.
export class Decimal {
  constructor(
    /** Low 8 bytes of the i128 mantissa (little-endian). */
    public mantissaLow: i64,
    /** High 8 bytes of the i128 mantissa (little-endian, sign-extended). */
    public mantissaHigh: i64,
    /** Decimal scale (number of fractional digits); valid range is 0..=28. */
    public scale: i32
  ) {}

  /**
   * Builds a `Decimal` whose mantissa fits in a single i64 (the common case for a
   * small, hand-written default such as `0.001`). The high half is the sign
   * extension of the low half so the two-half form still encodes the exact i128.
   */
  static fromI64(mantissa: i64, scale: i32): Decimal {
    // Arithmetic-shift the sign bit across the high half: a negative mantissa needs
    // all-ones in the high 8 bytes to be the correct two's-complement i128.
    const high: i64 = mantissa >> 63;
    return new Decimal(mantissa, high, scale);
  }
}
