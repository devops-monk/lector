//! The macOS Services menu entry point.
//!
//! Right-click selected text in any Cocoa app and macOS hands us the text
//! directly, through a pasteboard it owns. That is strictly better than the
//! hotkey path: no Accessibility permission, no synthesized keystroke, and no
//! clipboard to clobber and restore. It is also less discoverable, which is why
//! both exist.
//!
//! Registration is declarative, in Info.plist. This file is only the handler.

use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{MainThreadMarker, NSString};

use crate::App;

/// The Services callback is a C entry point with no user data we control, so the
/// app has to be reachable from a static. There is exactly one App per process.
static APP: OnceLock<std::sync::Arc<App>> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LectorServiceProvider"]
    #[derive(Debug)]
    struct ServiceProvider;

    impl ServiceProvider {
        #[unsafe(method(speakSelection:userData:error:))]
        fn speak_selection(
            &self,
            pboard: &NSPasteboard,
            _user_data: *const NSString,
            _error: *mut *const NSString,
        ) {
            let Some(app) = APP.get().cloned() else { return };
            let text = unsafe { pboard.stringForType(NSPasteboardTypeString) };
            let Some(text) = text else { return };
            let text = text.to_string();
            // Off the main thread: synthesis blocks, and this is the AppKit
            // thread that has to stay responsive.
            std::thread::spawn(move || app.speak(&text));
        }

        #[unsafe(method(stopSpeaking:userData:error:))]
        fn stop_speaking(
            &self,
            _pboard: &NSPasteboard,
            _user_data: *const NSString,
            _error: *mut *const NSString,
        ) {
            if let Some(app) = APP.get() {
                app.stop();
            }
        }
    }
);

impl ServiceProvider {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let _ = mtm;
        unsafe { msg_send![Self::alloc(), init] }
    }
}

/// Publishes the handler so the Services menu entries become live.
pub fn register(app: std::sync::Arc<App>) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("lector: services must be registered on the main thread");
        return;
    };
    let _ = APP.set(app);

    let provider = ServiceProvider::new(mtm);
    let ns_app = NSApplication::sharedApplication(mtm);
    unsafe {
        ns_app.setServicesProvider(Some(&provider));
        // Without this the menu entries do not appear until the next login.
        objc2_app_kit::NSUpdateDynamicServices();
    }
    // Deliberately leaked: the provider must outlive this call for the lifetime
    // of the process, and there is no other owner.
    std::mem::forget(provider);
}
