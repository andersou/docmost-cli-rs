fn main() {
    // Exposes the target triple so `self-update` can pick the matching
    // release archive (the names follow the CI packaging step).
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET")
    );
    println!("cargo:rerun-if-changed=build.rs");
}
