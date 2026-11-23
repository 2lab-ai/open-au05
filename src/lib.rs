pub mod config;
pub mod control;
pub mod daemon;
pub mod dictation;
pub mod hooks;
pub mod input;
pub mod protocol;
pub mod session;
pub mod system;
pub mod transport;
pub mod tui;

/// Compile-time version string matching the 2lab-ai tap contract:
/// `<pkg_version> (<channel> <rev12>)`.
/// `OPEN_AU05_BUILD_CHANNEL` defaults to `dev`; `OPEN_AU05_BUILD_ID` defaults to `unknown`.
pub fn version_info() -> String {
    let channel = option_env!("OPEN_AU05_BUILD_CHANNEL").unwrap_or("dev");
    let build_id = option_env!("OPEN_AU05_BUILD_ID").unwrap_or("unknown");
    format!("{} ({channel} {build_id})", env!("CARGO_PKG_VERSION"))
}
