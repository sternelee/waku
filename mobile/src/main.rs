//! Binary entry point for Waku Mobile.
//!
//! On **iOS** this provides `fn main()`, which delegates to
//! [`waku_mobile::ios_main`]. On **Android** the `android-activity` crate
//! invokes `android_main` from the cdylib — this binary is unused but must
//! compile for `cargo check` on the host.

#[cfg(target_os = "ios")]
fn main() {
    waku_mobile::ios_main();
}

#[cfg(target_os = "android")]
fn main() {
    eprintln!("Waku Mobile: android_main in lib.rs is the real entry point.");
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn main() {
    eprintln!(
        "Waku Mobile targets iOS and Android. \
         Build with --target aarch64-apple-ios-sim (iOS) or via cargo-ndk (Android)."
    );
}
