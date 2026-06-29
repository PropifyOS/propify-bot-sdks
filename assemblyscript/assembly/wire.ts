// The wire codec primitives, byte-for-byte compatible with the Rust
// `propify-sandbox-abi` codec.
//
// Everything is little-endian. wasm linear memory is little-endian and AS
// `load<T>`/`store<T>` are little-endian, so a multi-byte `store<i32>`/`store<i64>`
// at any offset writes the exact LE bytes the Rust `to_le_bytes` encoder produces.
// wasm permits unaligned accesses with identical results to aligned ones, so the
// variable-length `String` fields (which push later fields off natural alignment)
// are still encoded and decoded correctly.
//
// We work at the raw-pointer level (`load<u8>`/`store<u8>` and friends) rather than
// through the AS `String`/`Array` types on purpose: it keeps the import surface to
// exactly the four `propify` functions (the `String` type can pull UTF-8 machinery,
// and through it `abort`), and it lets a decoded asset alias the source buffer with
// zero copying.

import { Decimal } from "./decimal";

/**
 * A growable byte buffer in the guest's own linear memory, used to encode an
 * outgoing message.
 *
 * Backed by `heap.alloc`; it grows by allocating a larger block and copying. Under
 * the `stub` runtime `heap.free` is a no-op, but every block is still released via
 * [`Writer#free`] so the alloc/dealloc pairing is explicit and the code stays
 * correct under any allocator.
 */
export class Writer {
  private ptr: usize;
  private cap: i32;
  private len: i32;

  constructor(initialCapacity: i32) {
    this.cap = initialCapacity > 0 ? initialCapacity : 16;
    this.ptr = heap.alloc(<usize>this.cap);
    this.len = 0;
  }

  /** Number of bytes written so far. */
  get length(): i32 {
    return this.len;
  }

  /** Base offset of the encoded bytes in linear memory. */
  get pointer(): usize {
    return this.ptr;
  }

  /** Releases the backing block. Pairs with the constructor's allocation. */
  free(): void {
    heap.free(this.ptr);
  }

  private ensure(extra: i32): void {
    const need = this.len + extra;
    if (need <= this.cap) return;
    let newCap = this.cap * 2;
    while (newCap < need) newCap *= 2;
    const newPtr = heap.alloc(<usize>newCap);
    memory.copy(newPtr, this.ptr, <usize>this.len);
    heap.free(this.ptr);
    this.ptr = newPtr;
    this.cap = newCap;
  }

  putU8(value: u8): void {
    this.ensure(1);
    store<u8>(this.ptr + <usize>this.len, value);
    this.len += 1;
  }

  putU32(value: u32): void {
    this.ensure(4);
    store<u32>(this.ptr + <usize>this.len, value);
    this.len += 4;
  }

  putI32(value: i32): void {
    this.ensure(4);
    store<i32>(this.ptr + <usize>this.len, value);
    this.len += 4;
  }

  putI64(value: i64): void {
    this.ensure(8);
    store<i64>(this.ptr + <usize>this.len, value);
    this.len += 8;
  }

  putBool(value: bool): void {
    this.putU8(value ? <u8>1 : <u8>0);
  }

  /** Copies `len` raw bytes from `srcPtr` into the buffer. */
  putBytes(srcPtr: usize, len: i32): void {
    if (len <= 0) return;
    this.ensure(len);
    memory.copy(this.ptr + <usize>this.len, srcPtr, <usize>len);
    this.len += len;
  }

  /** Encodes a `String` field: a u32 LE byte length, then the raw bytes. */
  putString(srcPtr: usize, len: i32): void {
    this.putU32(<u32>len);
    this.putBytes(srcPtr, len);
  }

  /** Encodes a `Decimal`: i128 mantissa (16 LE, two i64 halves) + i32 scale (4 LE). */
  putDecimal(value: Decimal): void {
    this.putI64(value.mantissaLow);
    this.putI64(value.mantissaHigh);
    this.putI32(value.scale);
  }

  /** Encodes an `Option<Decimal>`: tag 0 = None, tag 1 = Some + the decimal. */
  putOptionDecimal(value: Decimal | null): void {
    if (value === null) {
      this.putU8(0);
    } else {
      this.putU8(1);
      this.putDecimal(value);
    }
  }
}

/**
 * A bounds-checked, forward-only reader over a byte range in linear memory.
 *
 * Mirrors the Rust codec's `Cursor`: every read first checks that enough bytes
 * remain and flips [`Reader#failed`] instead of reading out of bounds. A caller
 * inspects `failed` after a sequence of reads to decide whether the decode held.
 * The source bytes are the host-written input buffer, which the SDK keeps alive for
 * the whole tick, so decoded byte slices may alias it safely.
 */
export class Reader {
  private base: usize;
  private end: usize;
  private pos: usize;
  /** Set once any read ran past the end; the decoded value is then meaningless. */
  failed: bool;

  constructor(ptr: usize, len: i32) {
    const safeLen: i32 = len > 0 ? len : 0;
    this.base = ptr;
    this.end = ptr + <usize>safeLen;
    this.pos = ptr;
    this.failed = false;
  }

  private has(n: usize): bool {
    return !this.failed && this.end - this.pos >= n;
  }

  /**
   * The reader's current absolute pointer into linear memory. Used by
   * [`MarketWindow#decode`] to record where the candle array begins so an individual
   * candle can be read in place (copy-free) on demand.
   */
  get position(): usize {
    return this.pos;
  }

  /** Bytes left to read between the cursor and the end of the buffer. */
  get remaining(): i32 {
    return <i32>(this.end - this.pos);
  }

  readU8(): u8 {
    if (!this.has(1)) {
      this.failed = true;
      return 0;
    }
    const v = load<u8>(this.pos);
    this.pos += 1;
    return v;
  }

  readU32(): u32 {
    if (!this.has(4)) {
      this.failed = true;
      return 0;
    }
    const v = load<u32>(this.pos);
    this.pos += 4;
    return v;
  }

  readI32(): i32 {
    if (!this.has(4)) {
      this.failed = true;
      return 0;
    }
    const v = load<i32>(this.pos);
    this.pos += 4;
    return v;
  }

  readI64(): i64 {
    if (!this.has(8)) {
      this.failed = true;
      return 0;
    }
    const v = load<i64>(this.pos);
    this.pos += 8;
    return v;
  }

  readBool(): bool {
    return this.readU8() != 0;
  }

  /**
   * Reads a `String` field and returns its byte range as a [`ByteSlice`] aliasing
   * the source buffer (no copy). The length prefix is validated against the
   * remaining bytes.
   */
  readString(): ByteSlice {
    const len = this.readU32();
    if (this.failed || !this.has(<usize>len)) {
      this.failed = true;
      return new ByteSlice(0, 0);
    }
    const slice = new ByteSlice(this.pos, <i32>len);
    this.pos += <usize>len;
    return slice;
  }

  /** Reads a `Decimal`: i128 mantissa (16 LE, two i64 halves) + i32 scale (4 LE). */
  readDecimal(): Decimal {
    const low = this.readI64();
    const high = this.readI64();
    const scale = this.readI32();
    return new Decimal(low, high, scale);
  }

  /** Advances over a `Decimal` (20 bytes) without materializing it. */
  skipDecimal(): void {
    this.readI64();
    this.readI64();
    this.readI32();
  }
}

/**
 * A borrowed view of bytes in linear memory: a base pointer and a length.
 *
 * Used for `String` fields so an asset symbol can be passed straight through from a
 * decoded `MarketSnapshot` into an outgoing `OrderIntentBody` without ever building
 * an AS `String`.
 */
export class ByteSlice {
  constructor(public ptr: usize, public len: i32) {}
}

/**
 * Byte-exact equality between a [`ByteSlice`] and an ASCII literal held in a
 * `StaticArray<u8>`, used to look up a strategy parameter by name without UTF-8
 * decoding either side.
 */
export function sliceEqualsAscii(slice: ByteSlice, ascii: StaticArray<u8>): bool {
  const len = ascii.length;
  if (slice.len != len) return false;
  for (let i = 0; i < len; i++) {
    if (load<u8>(slice.ptr + <usize>i) != unchecked(ascii[i])) return false;
  }
  return true;
}
