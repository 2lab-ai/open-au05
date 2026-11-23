fn main() {
    println!("cargo:rerun-if-env-changed=OPEN_AU05_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPEN_AU05_BUILD_ID");

    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let channel = std::env::var("OPEN_AU05_BUILD_CHANNEL").unwrap_or_else(|_| "dev".into());
    let build_id = std::env::var("OPEN_AU05_BUILD_ID").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=OPEN_AU05_VERSION_STRING={pkg} ({channel} {build_id})");
}
