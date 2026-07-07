fn main() {
    // Expose the exact target triple so the crate can pick the matching
    // release asset. Cargo sets `TARGET` for build scripts.
    let target = std::env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    println!("cargo:rustc-env=BUILD_TARGET={target}");
}
