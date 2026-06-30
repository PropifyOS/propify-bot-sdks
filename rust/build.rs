//! Build script that emits the canonical `propify_manifest` bytes for the bundled
//! `minimal_bot` example.
//!
//! ABI v3 carries the bot manifest inside the artifact as a `propify_manifest` custom
//! section. In Rust the section is emitted natively from a `#[link_section]` static (see
//! the `declare_manifest!` macro), and the bytes it embeds must be the canonical
//! `BotManifest::encode` output so the artifact is byte-identical on rebuild. This script
//! builds that manifest and writes the encoded bytes to `$OUT_DIR/propify_manifest.bin`,
//! which `examples/minimal_bot.rs` embeds via `declare_manifest!`.
//!
//! It lives on the SDK package only to support that bundled example and to be the exact
//! pattern a downstream bot crate copies into its own `build.rs` (with its own manifest
//! values). It is cheap: it writes one small file and does no codegen.

use std::env;
use std::fs;
use std::path::Path;

use propify_sandbox_abi::BotManifest;

fn main() {
    // The manifest for the bundled minimal example. A real bot crate copies this shape
    // into its own build script and fills in its own identity. Every field is within its
    // ABI v3 per-field byte cap; the `author_erc20` is a valid EIP-55 checksummed address
    // (the semantic validators run host-side at submission, not here).
    let manifest = BotManifest {
        name: "Minimal Bot".to_string(),
        description: "A minimal starting-point PropifyOS bot: one market BUY per tick.".to_string(),
        version: "0.1.0".to_string(),
        license: "Apache-2.0".to_string(),
        image_sha256: None,
        author_name: "PropifyOS".to_string(),
        author_email: "bots@propifyos.app".to_string(),
        author_erc20: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
        source_repo_url: "https://github.com/PropifyOS/propify-bot-sdks".to_string(),
    };

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is always set for a build script");
    let out_path = Path::new(&out_dir).join("propify_manifest.bin");
    fs::write(&out_path, manifest.encode()).expect("writing the manifest bytes must succeed");

    // Only the build script affects the emitted bytes, so rebuild the file only when it
    // changes rather than on every source edit.
    println!("cargo:rerun-if-changed=build.rs");
}
