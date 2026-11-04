//! `cargo run -p cc-virtualcam --example activate` — submits the camera
//! extension activation request and prints state changes (macOS only).

#[cfg(target_os = "macos")]
fn main() {
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
    }
    cc_virtualcam::macos::activate();
    let mut last = None;
    for _ in 0..50 {
        // SAFETY: runs the main run loop so main-queue callbacks are delivered.
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0) };
        let state = cc_virtualcam::macos::activation_state();
        if last.as_ref() != Some(&state) {
            println!("{state:?}");
            last = Some(state);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
