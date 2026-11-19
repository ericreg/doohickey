#[cfg(target_os = "macos")]
mod app;
#[cfg(any(target_os = "macos", test))]
mod store;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("doohickey requires macOS.");
    std::process::ExitCode::FAILURE
}
