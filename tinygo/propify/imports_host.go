//go:build !tinygo

// Host-only stubs for the six `propify` capability imports.
//
// The real bindings live in imports.go, which is tagged `//go:build tinygo` and
// declares these same six functions bodyless via `//go:wasmimport propify ...`.
// The standard (non-TinyGo) Go toolchain rejects a bodyless function ("missing
// function body"), so without this file `go test ./...` could not even build the
// package on the host — which would block the pure byte-parser decoder tests
// (decodeMarketWindow and friends) from ever running in-tree or in CI.
//
// These trivial `return 0` bodies exist ONLY so the package compiles off-TinyGo
// and its host-independent decoders stay unit-testable. They are never linked into
// the wasm guest: a TinyGo build (its build-tag set includes `tinygo`) excludes
// this file via `//go:build !tinygo` and the guest uses the real `//go:wasmimport`
// bindings in imports.go instead. The `tinygo` tag — not `wasm` — is the correct
// distinguisher because TinyGo's `wasm-unknown` target reports `GOARCH=arm`, so a
// `wasm` constraint would wrongly select these stubs into the guest. The signatures
// here match imports.go exactly so the same call sites compile on both build paths.
package propify

func hostReadMarketData(ptr int32, length int32) int32 { return 0 }

func hostReadMarketWindow(ptr int32, length int32) int32 { return 0 }

func hostReadStrategyParams(ptr int32, length int32) int32 { return 0 }

func hostReadAccountView(ptr int32, length int32) int32 { return 0 }

func hostReadAccountContext(ptr int32, length int32) int32 { return 0 }

func hostEmitIntent(ptr int32, length int32) int32 { return 0 }
