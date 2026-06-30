//! `manifest-encoder` — the canonical `propify_manifest` encoder and section injector
//! for the non-Rust SDKs.
//!
//! ABI v3 carries the bot manifest inside the artifact as a `propify_manifest` wasm
//! custom section. The Rust SDK emits that section natively from a `#[link_section]`
//! static, but AssemblyScript and TinyGo have no portable link-section attribute, so the
//! SDK build injects the section after the build. The injected bytes MUST be canonical —
//! produced by [`propify_sandbox_abi::BotManifest::encode`], the single shared codec — so
//! this tool is the one place that encoding happens. The Go and AssemblyScript SDKs do not
//! re-implement the manifest encoder.
//!
//! `wasm-tools` (1.252) has no add-custom-section subcommand (only `strip`, which
//! removes), so this tool implements the minimal, correct custom-section append itself.
//!
//! # Commands
//!
//! - `encode <descriptor.json> <out.bin>` — read a manifest descriptor and write the
//!   canonical encoded manifest bytes.
//! - `inject <in.wasm> <manifest.bin> <out.wasm>` — append a `propify_manifest` custom
//!   section carrying those bytes to a wasm module.
//! - `verify <wasm>` — parse the module, assert exactly one `propify_manifest` section,
//!   and decode it with [`BotManifest::decode`], printing the result.
//!
//! The descriptor is a small JSON object mirroring [`BotManifest`]. `image_sha256` is an
//! optional 64-character hex string (absent or `null` when the bot ships no image):
//!
//! ```json
//! {
//!   "name": "Grid Bot",
//!   "description": "A grid trading strategy.",
//!   "version": "1.0.0",
//!   "license": "Apache-2.0",
//!   "image_sha256": null,
//!   "author_name": "Jane Doe",
//!   "author_email": "jane@example.com",
//!   "author_erc20": "0x52908400098527886E0F7030069857D2E4169EE7",
//!   "source_repo_url": "https://example.com/jane/grid-bot"
//! }
//! ```

use std::error::Error;
use std::fs;
use std::process::ExitCode;

use propify_sandbox_abi::BotManifest;
use serde::Deserialize;

/// The wasm custom-section name the host extracts and validates.
const MANIFEST_SECTION: &str = "propify_manifest";

/// A JSON manifest descriptor, mapped one-to-one onto a [`BotManifest`].
///
/// `image_sha256` is the only field that is not a plain `BotManifest` string: it is an
/// optional 64-hex-character content hash that maps onto the `Option<[u8; 32]>`. Unknown
/// fields are rejected so a typo in the descriptor is caught rather than silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDescriptor {
    name: String,
    description: String,
    version: String,
    license: String,
    #[serde(default)]
    image_sha256: Option<String>,
    author_name: String,
    author_email: String,
    author_erc20: String,
    source_repo_url: String,
}

impl ManifestDescriptor {
    /// Builds the [`BotManifest`], parsing the optional image hex into the 32-byte hash.
    fn into_manifest(self) -> Result<BotManifest, Box<dyn Error>> {
        let image_sha256 = match self.image_sha256 {
            None => None,
            Some(hex) => Some(parse_hex32(&hex)?),
        };
        Ok(BotManifest {
            name: self.name,
            description: self.description,
            version: self.version,
            license: self.license,
            image_sha256,
            author_name: self.author_name,
            author_email: self.author_email,
            author_erc20: self.author_erc20,
            source_repo_url: self.source_repo_url,
        })
    }
}

/// Parses a 64-character hex string into a 32-byte array, rejecting any other length or a
/// non-hex digit. This is a structural parse only; the manifest's semantic image checks
/// (dimensions, format, hash match) run marketplace-side, not here.
fn parse_hex32(hex: &str) -> Result<[u8; 32], Box<dyn Error>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() != 64 {
        return Err(format!("image_sha256 must be 64 hex characters, got {}", hex.len()).into());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = &hex[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(pair, 16)
            .map_err(|_| format!("image_sha256 has a non-hex byte at position {}", i * 2))?;
    }
    Ok(out)
}

/// Appends a `propify_manifest` custom section carrying `payload` to a wasm module.
///
/// A wasm custom section is `section_id = 0`, then a ULEB128 size, then the section body:
/// a ULEB128 name length, the name bytes, and the payload. Custom sections are valid
/// anywhere in the module, so appending one to the end of an otherwise-valid module yields
/// a valid module. The input is checked for the wasm magic so a non-wasm file fails with a
/// clear message rather than producing garbage.
fn append_custom_section(
    module: &[u8],
    name: &str,
    payload: &[u8],
) -> Result<Vec<u8>, Box<dyn Error>> {
    if module.len() < 8 || &module[0..4] != b"\0asm" {
        return Err("input is not a WebAssembly module (missing \\0asm magic)".into());
    }
    let mut body = Vec::new();
    write_uleb128(&mut body, name.len() as u64);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);

    let mut out = module.to_vec();
    out.push(0); // custom section id
    write_uleb128(&mut out, body.len() as u64);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Writes an unsigned LEB128 integer, the variable-length encoding wasm uses for section
/// sizes and name lengths.
fn write_uleb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Finds every `propify_manifest` custom section in a module, returning their payloads.
///
/// A streaming `wasmparser` pass, the same kind of read the host scanner does: it never
/// instantiates or runs the module. The caller asserts the count is exactly one.
fn find_manifest_sections(module: &[u8]) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut found = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(module) {
        if let wasmparser::Payload::CustomSection(reader) = payload?
            && reader.name() == MANIFEST_SECTION
        {
            found.push(reader.data().to_vec());
        }
    }
    Ok(found)
}

/// `encode <descriptor.json> <out.bin>`: descriptor -> canonical manifest bytes.
fn cmd_encode(descriptor_path: &str, out_path: &str) -> Result<(), Box<dyn Error>> {
    let json = fs::read_to_string(descriptor_path)
        .map_err(|e| format!("reading {descriptor_path}: {e}"))?;
    let descriptor: ManifestDescriptor =
        serde_json::from_str(&json).map_err(|e| format!("parsing {descriptor_path}: {e}"))?;
    let manifest = descriptor.into_manifest()?;
    let bytes = manifest.encode();
    fs::write(out_path, &bytes).map_err(|e| format!("writing {out_path}: {e}"))?;
    println!(
        "wrote {} canonical manifest bytes to {out_path}",
        bytes.len()
    );
    Ok(())
}

/// `inject <in.wasm> <manifest.bin> <out.wasm>`: append the section.
fn cmd_inject(wasm_path: &str, manifest_path: &str, out_path: &str) -> Result<(), Box<dyn Error>> {
    let module = fs::read(wasm_path).map_err(|e| format!("reading {wasm_path}: {e}"))?;
    let payload = fs::read(manifest_path).map_err(|e| format!("reading {manifest_path}: {e}"))?;

    // Refuse to add a second section: the host rejects a module with more than one
    // `propify_manifest`, so fail loudly here instead of producing a module that the host
    // would reject.
    let existing = find_manifest_sections(&module)?;
    if !existing.is_empty() {
        return Err(format!(
            "{wasm_path} already has {} propify_manifest section(s); refusing to add another",
            existing.len()
        )
        .into());
    }

    let out = append_custom_section(&module, MANIFEST_SECTION, &payload)?;
    fs::write(out_path, &out).map_err(|e| format!("writing {out_path}: {e}"))?;
    println!(
        "injected a {}-byte {MANIFEST_SECTION} section into {out_path}",
        payload.len()
    );
    Ok(())
}

/// `verify <wasm>`: exactly one `propify_manifest` section that decodes cleanly.
fn cmd_verify(wasm_path: &str) -> Result<(), Box<dyn Error>> {
    let module = fs::read(wasm_path).map_err(|e| format!("reading {wasm_path}: {e}"))?;
    let sections = find_manifest_sections(&module)?;
    match sections.len() {
        0 => return Err(format!("{wasm_path} has no {MANIFEST_SECTION} section").into()),
        1 => {}
        n => {
            return Err(
                format!("{wasm_path} has {n} {MANIFEST_SECTION} sections, expected 1").into(),
            );
        }
    }
    let manifest = BotManifest::decode(&sections[0])
        .map_err(|e| format!("the {MANIFEST_SECTION} section did not decode: {e}"))?;
    println!("{wasm_path}: exactly one {MANIFEST_SECTION} section, decodes cleanly:");
    println!("  name:            {}", manifest.name);
    println!("  version:         {}", manifest.version);
    println!("  license:         {}", manifest.license);
    println!("  author_name:     {}", manifest.author_name);
    println!("  source_repo_url: {}", manifest.source_repo_url);
    println!("  has_image:       {}", manifest.image_sha256.is_some());
    Ok(())
}

/// Prints usage to stderr.
fn usage() {
    eprintln!(
        "manifest-encoder — canonical propify_manifest encoder and section injector\n\
         \n\
         USAGE:\n  \
           manifest-encoder encode <descriptor.json> <out.bin>\n  \
           manifest-encoder inject <in.wasm> <manifest.bin> <out.wasm>\n  \
           manifest-encoder verify <wasm>"
    );
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, descriptor, out] if cmd == "encode" => cmd_encode(descriptor, out),
        [cmd, wasm, manifest, out] if cmd == "inject" => cmd_inject(wasm, manifest, out),
        [cmd, wasm] if cmd == "verify" => cmd_verify(wasm),
        _ => {
            usage();
            Err("invalid arguments".into())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
