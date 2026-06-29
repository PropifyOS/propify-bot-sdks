//! The wasm32 FFI shim: the *only* place this crate uses `unsafe`.
//!
//! This module is compiled solely for `target_arch = "wasm32"` (the sandbox target).
//! It declares the five host capability imports, provides the `std::alloc`-backed
//! allocator the ABI exports, and implements [`HostBindings`] by reading and
//! writing the guest's own linear memory through raw pointers. A single
//! module-scoped `#[allow(unsafe_code)]` permits the `unsafe` blocks here while the
//! crate stays on `unsafe_code = "deny"` everywhere else; every block carries a
//! `// SAFETY:` note. The guest is single-threaded (the host denies the wasm threads
//! proposal), so none of these pointer operations races another.
#![allow(unsafe_code)]

use core::alloc::Layout;

use crate::bot::HostBindings;

/// Alignment for every guest buffer this SDK allocates.
///
/// Wire buffers are plain `u8` arrays, so a modest, universally valid alignment is
/// enough. It is fixed (not stored per allocation) so `dealloc` can reconstruct the
/// exact `Layout` from just `(ptr, size)`, which is all the host hands back.
const BUFFER_ALIGN: usize = 8;

// The capability functions the host grants, all in the `propify` namespace and all
// `(ptr: i32, len: i32) -> i32`. The `unsafe extern` block (required in edition 2024)
// declares them; calling them is `unsafe`. `host_read_market_window` is the ABI v2
// addition and sits alongside the snapshot, params, and account reads.
#[link(wasm_import_module = "propify")]
unsafe extern "C" {
    fn host_read_market_data(ptr: i32, len: i32) -> i32;
    fn host_read_market_window(ptr: i32, len: i32) -> i32;
    fn host_read_strategy_params(ptr: i32, len: i32) -> i32;
    fn host_read_account_view(ptr: i32, len: i32) -> i32;
    fn host_emit_intent(ptr: i32, len: i32) -> i32;
}

/// Reserves `size` bytes in the guest's linear memory and returns the offset, or `0`
/// on any failure. Backs the `alloc` export.
///
/// The host never calls this (it only writes into buffers the guest has already
/// reserved), but the export must exist with this signature to satisfy the load
/// check. A `0` (null-like) return on a bad size or allocation failure is propagated
/// rather than panicking.
#[must_use]
pub fn wasm_alloc(size: i32) -> i32 {
    let Ok(size) = usize::try_from(size) else {
        return 0;
    };
    if size == 0 {
        return 0;
    }
    let Ok(layout) = Layout::from_size_align(size, BUFFER_ALIGN) else {
        return 0;
    };
    // SAFETY: `layout` has a strictly positive size, the documented precondition of
    // `std::alloc::alloc`. A null return (allocation failure) is surfaced to the
    // caller as `0`; it is never dereferenced here.
    let ptr = unsafe { std::alloc::alloc(layout) };
    ptr as usize as i32
}

/// Releases a buffer previously returned by [`wasm_alloc`]. Backs the `dealloc`
/// export.
pub fn wasm_dealloc(ptr: i32, size: i32) {
    let (Ok(addr), Ok(size)) = (usize::try_from(ptr), usize::try_from(size)) else {
        return;
    };
    if addr == 0 || size == 0 {
        return;
    }
    let Ok(layout) = Layout::from_size_align(size, BUFFER_ALIGN) else {
        return;
    };
    // SAFETY: `addr`/`size` describe a block this SDK obtained from `wasm_alloc` with
    // this exact size and `BUFFER_ALIGN` (the tick driver pairs every alloc with one
    // dealloc of the same size), so the layout matches the live allocation and it is
    // freed exactly once.
    unsafe { std::alloc::dealloc(addr as *mut u8, layout) };
}

/// The real host binding: calls the `propify` imports and accesses the guest's own
/// linear memory directly.
pub struct WasmHost;

impl HostBindings for WasmHost {
    fn read_market_data(&mut self, ptr: u32, len: u32) -> i32 {
        // SAFETY: calling a host-provided import. Per the read protocol it only reads
        // `len` bytes of our linear memory at `ptr` (or writes the snapshot there)
        // and returns a length/status; no Rust aliasing invariant is involved.
        unsafe { host_read_market_data(ptr as i32, len as i32) }
    }

    fn read_market_window(&mut self, ptr: u32, len: u32) -> i32 {
        // SAFETY: as above; the ABI v2 host import operating on our buffer at
        // `(ptr, len)`, writing the encoded window there and returning a length/status.
        unsafe { host_read_market_window(ptr as i32, len as i32) }
    }

    fn read_strategy_params(&mut self, ptr: u32, len: u32) -> i32 {
        // SAFETY: as above; a host import operating on our buffer at `(ptr, len)`.
        unsafe { host_read_strategy_params(ptr as i32, len as i32) }
    }

    fn read_account_view(&mut self, ptr: u32, len: u32) -> i32 {
        // SAFETY: as above; a host import operating on our buffer at `(ptr, len)`.
        unsafe { host_read_account_view(ptr as i32, len as i32) }
    }

    fn emit_intent(&mut self, ptr: u32, len: u32) -> i32 {
        // SAFETY: as above; the host reads `len` encoded bytes from our buffer at
        // `ptr` and returns a status. The buffer was filled by `store` just before.
        unsafe { host_emit_intent(ptr as i32, len as i32) }
    }

    fn alloc(&mut self, size: u32) -> u32 {
        wasm_alloc(size as i32) as u32
    }

    fn dealloc(&mut self, ptr: u32, size: u32) {
        wasm_dealloc(ptr as i32, size as i32);
    }

    fn load(&self, ptr: u32, len: u32) -> Vec<u8> {
        if len == 0 {
            return Vec::new();
        }
        // SAFETY: `ptr`/`len` name a buffer this SDK allocated (via `alloc`) and the
        // host has just filled with exactly `len` initialized bytes. It lies within
        // our own linear memory, `u8` has alignment 1, and the single-threaded guest
        // performs no concurrent mutation for the duration of this read.
        unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize).to_vec() }
    }

    fn store(&mut self, ptr: u32, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: `ptr` came from `alloc(bytes.len())`, so the destination owns at
        // least `bytes.len()` bytes within our linear memory; `u8` has alignment 1;
        // source (a `Vec`) and destination (a fresh allocation) do not overlap, which
        // is the precondition of `copy_nonoverlapping`.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as usize as *mut u8, bytes.len());
        }
    }
}
