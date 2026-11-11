//! Move a file or folder to the system Trash, where it can be put back.

use std::path::Path;

#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    // SAFETY: plain calls with an owned path string, a valid file URL, and
    // no result URL wanted.
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None)
    }
    .map_err(|e| format!("cannot move to the Trash: {}", e.localizedDescription()))
}

// ponytail: macOS only; other platforms refuse rather than delete for good.
#[cfg(not(target_os = "macos"))]
pub fn move_to_trash(_: &Path) -> Result<(), String> {
    Err("moving to the Trash is only supported on macOS".into())
}
