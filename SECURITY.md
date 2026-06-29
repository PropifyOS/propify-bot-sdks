# Security policy

This repository holds the public guest SDKs and the shared wire contract (the
`abi/` crate) for the PropifyOS bot sandbox. It contains guest-side code only. It
holds no credentials, no server code, and no access to any PropifyOS production
system.

## Reporting a vulnerability

If you find a security issue in any SDK or in the ABI codec, report it privately.
Do not open a public issue, a pull request, or a discussion for a vulnerability,
because that discloses it before a fix is available.

The primary path is to open a private advisory through this repo's Security tab
(the "Report a vulnerability" button). GitHub keeps the report private until a
fix is coordinated and a disclosure timeline is agreed.

If you cannot use the Security tab, email security@propifyos.app instead.

Please include:

- The affected SDK or component (Rust, AssemblyScript, TinyGo, or the `abi` crate).
- A description of the issue and its impact.
- Steps to reproduce, a proof of concept, or the affected source location.
- The version, commit, or release tag you tested against.

You will receive an acknowledgement that the report was received. We will
investigate, keep you informed of progress, and coordinate a disclosure timeline
with you once a fix is ready.

## Scope

In scope:

- Memory-safety or soundness defects in an SDK's guest code.
- Codec defects in the `abi` crate that let a malformed message decode incorrectly
  or escape the documented bounds.
- Any path by which guest SDK code could reach a capability outside the documented
  ABI surface.

Out of scope for this repository:

- The sandbox host itself and any PropifyOS production service. They are not part
  of this repository. A report about the host belongs to the same private channel
  above, marked as host-related.
- Trading outcomes. The example bots are reference code, not financial advice, and
  are not tuned to any individual's risk.

## A note on the sandbox model

The host treats every guest module as untrusted. The documented limits (the fuel
budget, the 16 MiB linear-memory cap, the bounded candle window, and the empty
linker that grants only the `propify` capabilities) are deliberate boundaries, not
defects. A report that a guest cannot read a file, open a socket, see a clock, or
exceed these limits describes the sandbox working as designed. See `docs/` for the
full model.
