use std::fs;

fn main() {
    // Ensure the linker can find memory.x in the crate root.
    println!(
        "cargo:rustc-link-search={}",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );

    // Re-run this build script if memory.x changes.
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    // ---------------------------------------------------------------------------
    // Embed build-time identity for crash reports.
    // ---------------------------------------------------------------------------
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // Git commit hash — first 4 bytes (8 hex chars) as u32.
    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let sha8 = git_sha.trim();
    let sha8 = if sha8.len() >= 8 { &sha8[..8] } else { "00000000" };
    let git_hash_u32 = u32::from_str_radix(sha8, 16).unwrap_or(0);

    // Firmware version from Cargo.toml.
    let pkg_ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let parts: Vec<u8> = pkg_ver
        .split('.')
        .map(|s| s.parse::<u8>().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );

    fs::write(
        format!("{out_dir}/crash_build_info.rs"),
        format!(
            "/// First 4 bytes of the git commit SHA at build time.\n\
             pub const GIT_HASH_U32: u32 = 0x{git_hash_u32:08x};\n\
             /// Firmware semantic version baked in at build time.\n\
             pub const FW_VERSION: [u8; 3] = [{major}, {minor}, {patch}];\n"
        ),
    )
    .expect("failed to write crash_build_info.rs");
}
