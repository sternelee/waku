//! Waku Mobile — native iOS/Android client for a Waku daemon host.
//!
//! The phone is a pure remote client: it dials the owner's daemon over iroh,
//! reads the shared session catalog, streams driver events for the open chat,
//! and sends prompts and control commands. Providers, Git, and files all run
//! on the daemon host.
//!
//! Entry points:
//! - **Android**: `android_main` (called by `android-activity` from the cdylib).
//! - **iOS**: the `main.rs` binary calls [`ios_main`], which registers the
//!   root view and hands control to the gpui-mobile run loop.

extern crate gpui_mobile;

pub mod remote;
pub mod screens;
pub mod state;

#[cfg(any(target_os = "ios", target_os = "android"))]
use gpui::{App, WindowOptions};

#[cfg(target_os = "android")]
use gpui::Application;

// Used by the platform entry points below (`open_main_window`), which are
// cfg-gated per target; the host stub leaves them unused.
#[allow(unused_imports)]
use state::{WakuMobile, install_ime_callback};

// ═══════════════════════════════════════════════════════════════════════════
// Android entry point
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("waku-mobile"),
    );
    gpui_mobile::android::jni::install_panic_hook();
    log::info!("waku-mobile: android_main entered");

    // Registers the global AndroidApp + AndroidPlatform.
    gpui_mobile::android::jni::init_platform(&app);
    let shared = match gpui_mobile::android::jni::shared_platform() {
        Some(shared) => shared,
        None => {
            log::error!("waku-mobile: no shared platform — aborting");
            return;
        }
    };

    Application::with_platform(shared.into_rc()).run(|cx: &mut App| {
        log::info!("waku-mobile: Application::run — opening main window");
        open_main_window(cx);
    });
    log::info!("waku-mobile: Application::run returned");
}

// ═══════════════════════════════════════════════════════════════════════════
// iOS entry point
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_register_app() {
    let _ = log::set_logger(&NsLogLogger).map(|()| log::set_max_level(log::LevelFilter::Info));
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("WAKU PANIC: {info}");
        nslog(&msg);
    }));
    gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
        open_main_window(cx);
    }));
}

#[cfg(target_os = "ios")]
pub fn ios_main() {
    gpui_ios_register_app();
    gpui_mobile::ios::ffi::run_app();
}

#[cfg(target_os = "ios")]
struct NsLogLogger;

#[cfg(target_os = "ios")]
impl log::Log for NsLogLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let msg = format!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
            nslog(&msg);
        }
    }
    fn flush(&self) {}
}

#[cfg(target_os = "ios")]
fn nslog(msg: &str) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        unsafe extern "C" {
            fn NSLog(fmt: *mut AnyObject, ...);
        }
        let c_msg = std::ffi::CString::new(msg).unwrap_or_default();
        let ns_msg: *mut AnyObject = msg_send![class!(NSString), alloc];
        let ns_msg: *mut AnyObject = msg_send![ns_msg, initWithUTF8String: c_msg.as_ptr()];
        let c_fmt = std::ffi::CString::new("%@").unwrap_or_default();
        let ns_fmt: *mut AnyObject = msg_send![class!(NSString), alloc];
        let ns_fmt: *mut AnyObject = msg_send![ns_fmt, initWithUTF8String: c_fmt.as_ptr()];
        NSLog(ns_fmt, ns_msg);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared window creation
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(any(target_os = "ios", target_os = "android"))]
fn open_main_window(cx: &mut App) {
    log::info!("waku-mobile: opening main window");
    let entity = WakuMobile::new(cx);
    install_ime_callback(entity.clone());
    match cx.open_window(
        WindowOptions {
            window_bounds: None,
            ..Default::default()
        },
        |_, _cx| entity.clone(),
    ) {
        Ok(_) => {
            log::info!("waku-mobile: window opened");
            // Defer the persisted-ticket reconnect a tick so the window's
            // platform callbacks (active-status registration) settle first;
            // dialing immediately races gpui-mobile's launch sequence.
            let entity = entity.clone();
            cx.spawn(async move |app| {
                app.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                let _ = entity.update(app, |this, cx| {
                    this.reconnect_saved(cx);
                });
            })
            .detach();
        }
        Err(error) => {
            log::error!("waku-mobile: open_window failed: {error}");
        }
    }
}

/// Host stub so `cargo check` on the desktop toolchain succeeds; the app only
/// runs on iOS and Android.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn open_main_window(_cx: &mut gpui::App) {
    log::warn!("waku-mobile: desktop host build — nothing to run here");
}
