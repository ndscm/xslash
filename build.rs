//! Build script: bake an `xslash-helper` xslash entry into the `xslash` binary,
//! so `xslash --` resolves and runs the helper instead of trusting `$PATH`.

use ::std::collections::BTreeMap;
use ::std::path::Path;
use ::std::path::PathBuf;

use ::serde::Serialize;

// ---------------------------------------------------------------------------
// entry
// ---------------------------------------------------------------------------

/// A download source for the helper artifact; the baked entry needs only its URL.
#[derive(Serialize)]
struct MinimalXslashProvider {
    url: String,
}

/// One platform's block: the hashed artifact, where it lands, and its sources.
#[derive(Serialize)]
struct MinimalXslashPlatform {
    hash: &'static str,
    digest: String,
    size: u64,
    path: &'static str,
    providers: Vec<MinimalXslashProvider>,
}

/// Minimal mirror of the `xslash-helper` entry shape. The runtime parses it
/// back into the real `xslash::XslashEntry`, which this script can't reference
/// (same crate, not built yet).
#[derive(Serialize)]
struct MinimalXslashEntry {
    name: &'static str,
    #[serde(rename = "final")]
    is_final: bool,
    platforms: BTreeMap<String, MinimalXslashPlatform>,
}

// ---------------------------------------------------------------------------
// generate
// ---------------------------------------------------------------------------

/// The `<kernel>-<arch>` key for the target being compiled, matching
/// `entry::host_platform`'s mapping (macos -> darwin, x86_64 -> amd64, ...).
fn target_platform() -> String {
    let kernel = ::std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let kernel = match kernel.as_str() {
        "macos" => "darwin",
        other => other,
    };
    let arch = ::std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{kernel}-{arch}")
}

/// Build the entry JSON by hashing `bin`, or `None` if it can't be read.
fn build_helper_entry(bin: &Path) -> Option<String> {
    let bytes = ::std::fs::read(bin).ok()?;
    println!("cargo:rerun-if-changed={}", bin.display());

    let platform = target_platform();
    let mut hasher = ::blake3::Hasher::new();
    hasher.update(&bytes);
    let digest = hasher.finalize().to_hex().to_string();

    // GitHub release URL, pinned to this package's version and platform asset.
    let version = ::std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let url = format!(
        "https://github.com/ndscm/xslash/releases/download/v{version}/xslash-helper-{platform}"
    );

    let entry = MinimalXslashEntry {
        name: "xslash-helper",
        is_final: true,
        platforms: BTreeMap::from([(
            platform,
            MinimalXslashPlatform {
                hash: "blake3",
                digest,
                size: bytes.len() as u64,
                path: "xslash-helper",
                providers: vec![MinimalXslashProvider { url }],
            },
        )]),
    };
    Some(::serde_json::to_string(&entry).expect("serializing xslash-helper entry"))
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=XSLASH_HELPER_BIN");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let out =
        PathBuf::from(::std::env::var("OUT_DIR").expect("OUT_DIR")).join("xslash-helper.xslash");

    // Only release builds bake a real entry; otherwise write an empty file so
    // `xslash --` falls back to xslash-helper on `$PATH`.
    let entry = if ::std::env::var("PROFILE").as_deref() == Ok("release") {
        // Needs the already-built binary to hash (supplied by the two-pass build).
        match ::std::env::var("XSLASH_HELPER_BIN") {
            Ok(bin) if !bin.is_empty() => build_helper_entry(Path::new(&bin)).unwrap_or_default(),
            _ => String::new(),
        }
    } else {
        String::new()
    };

    ::std::fs::write(&out, entry).expect("writing xslash-helper entry");
}
