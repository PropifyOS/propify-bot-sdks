// The TinyGo guest SDK module for the PropifyOS bot sandbox.
//
// It is a self-contained module beside the AssemblyScript and Rust SDKs. It has no
// third-party dependencies on purpose: the guest must compile to a wasm module that
// imports ONLY the `propify` host functions, so the SDK stays inside the standard
// library (and only `unsafe`, for the linear-memory allocator).
//
// The module path is the public SDK repository path. A creator imports the `propify`
// package from `github.com/PropifyOS/propify-bot-sdks/tinygo/propify`.
module github.com/PropifyOS/propify-bot-sdks/tinygo

go 1.24
