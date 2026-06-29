// The wire codec primitives, byte-for-byte compatible with the Rust
// `propify-sandbox-abi` codec and the AssemblyScript SDK.
//
// Everything is little-endian, two's complement. We do every multi-byte read and
// write with MANUAL little-endian byte shifts rather than `encoding/binary`, because
// that package is marked failing under TinyGo and pulling it in could also widen the
// import surface. Manual shifts keep the dependency set to nothing and produce the
// exact bytes the Rust `to_le_bytes` encoder produces.
//
// We work over plain `[]byte` slices into the guest's own linear memory. A decoded
// `String` field is returned as a sub-slice that ALIASES the source buffer (no copy),
// so a bot can pass an asset symbol straight through from a decoded snapshot into an
// outgoing order. The host keeps the input buffer alive for the whole tick, so the
// alias is valid until the tick ends.
package propify

// Writer is an append-only byte buffer used to encode one outgoing message. It is backed
// by a fixed, full-length region of the guest's bump arena (see newWriter), sized to the
// host's MAX_MESSAGE_BYTES, and writes through a manual position cursor with plain indexed
// stores. It deliberately avoids Go's `append`, which lowers to `runtime.sliceAppend` and
// can call `runtime.alloc` — and any heap allocation traps on this target because the host
// never calls `_initialize` to bootstrap the allocator. The cursor never exceeds the
// reserved length, so no store ever needs to grow. The caller pins the finished slice into
// linear memory (see emitIntent) before handing its pointer to the host.
type Writer struct {
	buf []byte
	pos int
}

// newWriter returns a Writer over a full-length, max-message-sized arena reservation, with
// the cursor at zero. Stores index into that fixed region, so no encode ever reaches the Go
// heap. If the arena cannot satisfy the reservation the buffer is nil (length 0); every
// store then becomes a bounds-guarded no-op and emitIntent offers an empty message, which
// the host rejects — a safe degradation rather than a heap trap. In practice the arena is
// sized so this never happens.
func newWriter() Writer {
	_, region := allocBuffer(maxMessageBytes)
	return Writer{buf: region}
}

// Bytes returns the encoded bytes written so far (the reserved region sliced to the cursor).
func (w *Writer) Bytes() []byte {
	return w.buf[:w.pos]
}

// putByte writes one byte at the cursor and advances it. It is bounds-guarded: a write past
// the reserved region is dropped rather than allowed to panic (which would trap). The region
// is sized to the message cap, so this guard never fires for a valid message.
func (w *Writer) putByte(v uint8) {
	if w.pos < len(w.buf) {
		w.buf[w.pos] = v
		w.pos++
	}
}

// PutU8 writes a single byte.
func (w *Writer) PutU8(v uint8) {
	w.putByte(v)
}

// PutU32 writes a u32 in little-endian order.
func (w *Writer) PutU32(v uint32) {
	w.putByte(byte(v))
	w.putByte(byte(v >> 8))
	w.putByte(byte(v >> 16))
	w.putByte(byte(v >> 24))
}

// PutI32 writes an i32 in little-endian, two's-complement order.
func (w *Writer) PutI32(v int32) {
	w.PutU32(uint32(v))
}

// PutU64 writes a u64 in little-endian order.
func (w *Writer) PutU64(v uint64) {
	w.putByte(byte(v))
	w.putByte(byte(v >> 8))
	w.putByte(byte(v >> 16))
	w.putByte(byte(v >> 24))
	w.putByte(byte(v >> 32))
	w.putByte(byte(v >> 40))
	w.putByte(byte(v >> 48))
	w.putByte(byte(v >> 56))
}

// PutI64 writes an i64 in little-endian, two's-complement order.
func (w *Writer) PutI64(v int64) {
	w.PutU64(uint64(v))
}

// PutBool writes a bool as 1 (true) or 0 (false), matching the Rust codec.
func (w *Writer) PutBool(v bool) {
	if v {
		w.putByte(1)
	} else {
		w.putByte(0)
	}
}

// PutBytes writes raw bytes verbatim through the cursor.
func (w *Writer) PutBytes(b []byte) {
	for i := 0; i < len(b); i++ {
		w.putByte(b[i])
	}
}

// PutString encodes a String field: a u32 little-endian byte length, then the bytes.
func (w *Writer) PutString(b []byte) {
	w.PutU32(uint32(len(b)))
	w.PutBytes(b)
}

// PutDecimal encodes a Decimal: i128 mantissa (16 LE, two u64 halves) then i32 scale
// (4 LE). Low half first, high half second, matching the i128 little-endian layout.
func (w *Writer) PutDecimal(d Decimal) {
	w.PutU64(d.MantissaLow)
	w.PutU64(d.MantissaHigh)
	w.PutI32(d.Scale)
}

// PutOptionDecimal encodes an Option<Decimal>: tag 0 = None, tag 1 = Some + the
// decimal. A nil pointer is None.
func (w *Writer) PutOptionDecimal(d *Decimal) {
	if d == nil {
		w.PutU8(0)
		return
	}
	w.PutU8(1)
	w.PutDecimal(*d)
}

// Reader is a bounds-checked, forward-only reader over a byte slice in linear memory.
//
// It mirrors the Rust codec's `Cursor` and the AssemblyScript `Reader`: every read
// first checks that enough bytes remain and flips `failed` instead of reading out of
// bounds. A caller inspects `failed` after a sequence of reads to decide whether the
// decode held. Once `failed` is set, later reads return zero values and stay failed.
type Reader struct {
	buf    []byte
	pos    int
	failed bool
}

// sharedReader is the single Reader instance every decode reuses. Returning the address
// of a freshly built `&Reader{}` would make it escape onto the Go heap — TinyGo's escape
// analysis cannot prove the pointer stays local once it is passed to the pointer-receiver
// read methods — and any heap allocation traps on this target because the host never calls
// `_initialize`. A package-level static lives in the data/bss segment instead, needing no
// allocation and no runtime init.
//
// One shared instance is safe here: the guest is single-threaded and every decode is
// strictly sequential and fully consumed before the next begins (the market snapshot, then
// the account view, then each strategy-parameter lookup), so no two readers are ever live
// at once. Each NewReader call rebinds the buffer and resets the cursor and failed flag.
var sharedReader Reader

// NewReader binds the shared Reader to the given buffer (resetting its cursor and failed
// flag) and returns it. See sharedReader for why a single static instance is used rather
// than a heap allocation.
func NewReader(buf []byte) *Reader {
	sharedReader = Reader{buf: buf}
	return &sharedReader
}

// has reports whether at least n more bytes are available and the reader has not
// already failed. A negative n (from an out-of-range length prefix on a 32-bit guest)
// is treated as "not available" so a hostile length cannot drive an out-of-bounds
// slice.
func (r *Reader) has(n int) bool {
	return !r.failed && n >= 0 && len(r.buf)-r.pos >= n
}

func (r *Reader) readU8() uint8 {
	if !r.has(1) {
		r.failed = true
		return 0
	}
	v := r.buf[r.pos]
	r.pos++
	return v
}

func (r *Reader) readU32() uint32 {
	if !r.has(4) {
		r.failed = true
		return 0
	}
	b := r.buf[r.pos:]
	v := uint32(b[0]) | uint32(b[1])<<8 | uint32(b[2])<<16 | uint32(b[3])<<24
	r.pos += 4
	return v
}

func (r *Reader) readI32() int32 {
	return int32(r.readU32())
}

func (r *Reader) readU64() uint64 {
	if !r.has(8) {
		r.failed = true
		return 0
	}
	b := r.buf[r.pos:]
	v := uint64(b[0]) | uint64(b[1])<<8 | uint64(b[2])<<16 | uint64(b[3])<<24 |
		uint64(b[4])<<32 | uint64(b[5])<<40 | uint64(b[6])<<48 | uint64(b[7])<<56
	r.pos += 8
	return v
}

func (r *Reader) readI64() int64 {
	return int64(r.readU64())
}

func (r *Reader) readBool() bool {
	return r.readU8() != 0
}

// readString reads a String field and returns its bytes as a sub-slice aliasing the
// source buffer (no copy). The length prefix is validated against the remaining
// bytes; an over-long prefix fails the reader and returns nil.
func (r *Reader) readString() []byte {
	n := int(r.readU32())
	if r.failed || !r.has(n) {
		r.failed = true
		return nil
	}
	s := r.buf[r.pos : r.pos+n]
	r.pos += n
	return s
}

// readDecimal reads a Decimal: i128 mantissa (16 LE, two u64 halves) then i32 scale
// (4 LE).
func (r *Reader) readDecimal() Decimal {
	low := r.readU64()
	high := r.readU64()
	scale := r.readI32()
	return Decimal{MantissaLow: low, MantissaHigh: high, Scale: scale}
}

// bytesEqual reports byte-exact equality between two slices, used to look up a
// strategy parameter by name without any UTF-8 decoding on either side.
func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
