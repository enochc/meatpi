//! Desktop entry point. On Android the entry point is `android_main` in the
//! library, so this binary is a no-op there.

#[cfg(not(target_os = "android"))]
fn main() -> Result<(), slint::PlatformError> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    meatpi_app::run()
}

#[cfg(target_os = "android")]
fn main() {}
