// The guest's linear-memory allocator, backing the `alloc`/`dealloc` ABI exports and
// the SDK's own read/emit buffers.
//
// Why a static arena and not the Go heap. TinyGo's `wasm-unknown` target builds a
// WebAssembly *reactor*: it emits an `_initialize` export that a host is expected to
// call once after instantiation to bootstrap the Go runtime, including the leaking
// garbage collector's bump pointer. The host does NOT call `_initialize`
// (its contract is fixed: `abi_version` then `on_tick`, with `alloc`/`dealloc` only as
// signature-checked exports). So any Go heap allocation — `make([]byte, ...)`, a
// growing `append`, an escaping `&Struct{}` — reads an uninitialised allocator global
// and traps with `unreachable`. The fix is to never touch the Go heap from any export
// reachable without `_initialize`.
//
// Instead we bump-allocate out of a package-level fixed-size byte array. A global array
// lives in the module's data/bss segment, which wasm zero-initialises AT INSTANTIATION
// (no `_initialize` needed); the bump cursor is a zeroed global for the same reason.
// `&arena[i]` is a real offset into the single exported linear `memory`, so a pointer we
// hand the host is a valid address it can write into and read back, exactly as the old
// `make`-based allocator intended — minus the runtime dependence.
//
// Lifetime. The host re-instantiates the module for every tick (a fresh `Store` and
// instance per `run`), so the arena and cursor reset to a clean zeroed state each tick
// at instantiation. `resetArena` additionally rewinds the cursor at the start of every
// `on_tick`, so even if one instance were ever reused across ticks each tick starts from
// a clean, deterministic arena and cannot accumulate or exhaust it. `Dealloc` is a
// no-op: there is no per-allocation free in a bump allocator, and nothing needs one.
package propify

import "unsafe"

// arenaSize is the fixed capacity of the guest's bump arena, in bytes. MAX_MESSAGE_BYTES
// is 64 KiB, the ceiling on any single wire message. A worst-case v3 tick bumps five read
// buffers (market, window, params, account, context — each its 256-byte initial guess
// plus, when a message exceeds the guess, a retry alloc up to MAX_MESSAGE_BYTES, since the
// bump allocator does not reclaim the discarded guess) and the 64 KiB emit reservation.
// Even bounding both the window and the context retries at the full 64 KiB cap, the total
// (~5 × 256 + 2 × 65_536 + 65_536 ≈ 198 KiB) fits inside this 512 KiB arena with wide
// margin. It costs only reserved linear memory, which is cheap.
const arenaSize int32 = 512 * 1024

// maxMessageBytes mirrors the host's MAX_MESSAGE_BYTES (`crate::config`): the largest
// single wire message. The emit Writer reserves this much arena up front so its `append`
// never grows past its capacity into the Go heap.
const maxMessageBytes int32 = 64 * 1024

// arena is the bump-allocation backing store. As a package-level array it sits in the
// module's data/bss segment and is zero-initialised at instantiation, needing no runtime
// init. Every `alloc` and every emit buffer is a sub-slice of this array.
var arena [arenaSize]byte

// bump is the next free offset into arena. A zeroed global, so it starts at 0 at
// instantiation with no init code; `resetArena` rewinds it per tick.
var bump int32

// resetArena rewinds the bump cursor to the start of the arena. Called at the start of
// every tick so repeated ticks on the same instance are deterministic and never exhaust
// the arena. A fresh instantiation already zeroes `bump`; this makes the per-tick reset
// explicit and independent of instance reuse.
func resetArena() {
	bump = 0
}

// align8 rounds a positive byte count up to the next multiple of 8 so successive
// allocations stay 8-byte aligned. All wire access is byte-wise (manual shifts), so
// alignment is not required for correctness; it is kept for tidiness and to avoid
// straddling. Inputs are small and bounded, so this cannot overflow in practice.
func align8(n int32) int32 {
	return (n + 7) &^ 7
}

// allocBuffer reserves `size` bytes from the arena and returns both the linear-memory
// offset of the first byte and a slice over exactly those bytes (so the SDK can read the
// data the host writes there). It returns a zero offset and a nil slice on a non-positive
// size or when the arena cannot satisfy the request, mirroring the old contract.
func allocBuffer(size int32) (uintptr, []byte) {
	if size <= 0 || size > arenaSize {
		return 0, nil
	}
	aligned := align8(size)
	// Guard against running off the end of the arena (and against the additive overflow,
	// since bump and aligned are both small and bounded here).
	if aligned <= 0 || bump > arenaSize-aligned {
		return 0, nil
	}
	start := bump
	bump += aligned
	// Three-index slice: visible length `size`, capacity bounded to this allocation's
	// aligned span so an accidental `append` cannot silently spill into the next block.
	buf := arena[start : start+size : start+aligned]
	return uintptr(unsafe.Pointer(&arena[start])), buf
}

// pin returns the linear-memory offset of an already-filled arena slice, for the encode
// path where the bytes exist before a pointer is needed. The bytes are already in the
// guest's linear memory (the arena), so this is just the address of the first byte. An
// empty slice yields a zero offset.
func pin(buf []byte) uintptr {
	if len(buf) == 0 {
		return 0
	}
	return uintptr(unsafe.Pointer(&buf[0]))
}

// AbiVersion backs the `abi_version` export: the ABI major version this SDK targets
// (3, the single supported version). It matches `propify-sandbox-abi::ABI_VERSION`; the
// host accepts only guests reporting 3 (the v1/v2 dual-support path is dropped) and
// refuses any other value before a tick runs. v3 adds the read-only AccountContext and
// the embedded manifest section.
func AbiVersion() int32 {
	return 3
}

// Alloc backs the `alloc` export: reserve `size` bytes in linear memory and return the
// offset, or 0 on a non-positive size or an exhausted arena. Present for ABI export-
// surface conformance; in the read protocol the SDK self-allocates and the host
// writes into the offset it is handed rather than calling this directly.
func Alloc(size int32) int32 {
	ptr, _ := allocBuffer(size)
	return int32(ptr)
}

// Dealloc backs the `dealloc` export: a no-op. A bump allocator has no per-block free,
// and the host resets all guest memory by re-instantiating the module each tick. The
// parameters are accepted to match the ABI signature the host validates.
func Dealloc(ptr int32, size int32) {
	_ = ptr
	_ = size
}
