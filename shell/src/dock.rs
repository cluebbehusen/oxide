//! macOS dock polish for bare (non-bundled) dev runs.
//!
//! miniquad's `Conf::icon` path hands the dock a 64px bitmap, which the
//! dock upscales into fuzz next to real apps. This module hands AppKit
//! the full-resolution mark instead. The packaged .app gets the same
//! crispness from its icns; running it through here too is a no-op
//! visually. This is the shell's one sanctioned unsafe corner — a
//! standard AppKit message send, nothing more.

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

/// Hands the dock the 1024px mark. Must run on the main thread after
/// the window (and thus NSApplication) exists — macroquad's main body
/// qualifies.
pub fn set_dock_icon() {
    let png: &[u8] = include_bytes!("../../assets/icon/oxide_1024.png");
    unsafe {
        let data: *mut Object = msg_send![
            class!(NSData),
            dataWithBytes: png.as_ptr() as *const std::ffi::c_void
            length: png.len()
        ];
        if data.is_null() {
            return;
        }
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}
