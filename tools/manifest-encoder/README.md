# manifest-encoder

A host-side build tool that produces the canonical `propify_manifest` bytes and injects
the wasm custom section for the SDKs that have no native link-section attribute (the
AssemblyScript and TinyGo SDKs).

ABI v3 carries the bot manifest (`BotManifest`) inside the artifact as a `propify_manifest`
custom section, hashed together with the code by `ArtifactId = sha256(module bytes)`. The
Rust SDK emits the section natively from a `#[link_section]` static (see
`rust/README.md`), so it does not use this tool. AssemblyScript and TinyGo inject the
section after the build, and the injected bytes must be the canonical
`propify_sandbox_abi::BotManifest::encode` output so the artifact is byte-identical on
rebuild. This tool is the single place that encoding happens; the Go and AssemblyScript
SDKs do not re-implement the manifest encoder.

`wasm-tools` 1.252 has no add-custom-section subcommand (only `strip`, which removes), so
this tool implements the minimal, correct custom-section append itself.

## Commands

```bash
manifest-encoder encode <descriptor.json> <out.bin>   # descriptor -> canonical bytes
manifest-encoder inject <in.wasm> <manifest.bin> <out.wasm>   # append the section
manifest-encoder verify <wasm>   # assert exactly one section, decode it
```

The descriptor is a small JSON object mirroring `BotManifest`; see
`manifest.example.json`. `image_sha256` is an optional 64-character hex string (absent or
`null` when the bot ships no image).

## Injecting the manifest into a TinyGo or AssemblyScript bot

After the SDK builds the guest `.wasm`, run:

```bash
# 1. Encode the descriptor to canonical manifest bytes.
cargo run -p manifest-encoder -- encode bot.manifest.json manifest.bin

# 2. Inject the propify_manifest custom section into the built module.
cargo run -p manifest-encoder -- inject build/sample.wasm manifest.bin build/sample.with-manifest.wasm

# 3. (Acceptance) Confirm exactly one section that decodes cleanly.
cargo run -p manifest-encoder -- verify build/sample.with-manifest.wasm
```

For a reproducible artifact, strip the `producers` section as the existing build does, then
inject the manifest last so the section bytes are deterministic:

```bash
wasm-tools strip -d producers build/sample.wasm -o build/sample.stripped.wasm
cargo run -p manifest-encoder -- inject build/sample.stripped.wasm manifest.bin build/sample.wasm
```

The injector refuses to add a second `propify_manifest` section, matching the host rule of
exactly one.

## Why not `wasm-tools`

`wasm-tools` can remove a custom section (`strip -d <name>`) but has no subcommand to add
one. Rather than depend on an external tool that cannot do the job, the append is a few
lines here: a custom section is `section_id = 0`, a ULEB128 size, then a ULEB128 name
length, the name, and the payload — valid anywhere in the module, so appended at the end.
The `verify` command uses `wasmparser` (the same streaming reader the host scanner uses) to
confirm the result.
