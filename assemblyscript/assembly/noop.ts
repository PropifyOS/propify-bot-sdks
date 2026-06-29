// The no-op `abort` the build routes to via `--use abort=assembly/noop/propifyAbort`.
//
// Why this exists: AssemblyScript's default `abort` lowers to an imported
// `env.abort` function. The sandbox ABI allows the guest exactly five
// imports (the `propify::host_*` capabilities) and the host REFUSES any module that
// imports anything else, `env.abort` included. Routing `abort` to this local
// function removes that import. Combined with `--noAssert` (which strips the
// assertion calls that would invoke `abort`) and our decoders being total
// (bounds-checked, never throwing), `abort` is never actually reached, so an empty
// body is correct: there is no error path that depends on it.
//
// Parameters are raw pointers/integers (`usize`/`u32`), deliberately NOT the AS
// `string` type: touching `string` here could pull UTF-8 machinery (and, through it,
// `abort`) back into the module and widen the import surface. We never read the
// arguments, so their pointer form is irrelevant to behaviour.
export function propifyAbort(
  message: usize,
  fileName: usize,
  line: u32,
  column: u32
): void {
  // Intentionally empty: see the module comment. Unreachable in practice.
}
