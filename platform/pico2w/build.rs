fn main() {
    // Ensure the linker can find memory.x in the crate root.
    println!("cargo:rustc-link-search={}", std::env::var("CARGO_MANIFEST_DIR").unwrap());

    // Re-run this build script if memory.x changes.
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
