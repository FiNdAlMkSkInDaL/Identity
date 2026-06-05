use std::io;

/// Write string content to the system clipboard using UTF-16 wide representation.
pub fn set_clipboard_text(text: &str) -> Result<(), io::Error> {
    #[cfg(windows)]
    {
        set_clipboard_text_windows(text)
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clipboard writing is only supported on Windows",
        ))
    }
}

/// Read UTF-16 text from the system clipboard.
pub fn get_clipboard_text() -> Result<String, io::Error> {
    #[cfg(windows)]
    {
        get_clipboard_text_windows()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clipboard reading is only supported on Windows",
        ))
    }
}

#[cfg(windows)]
fn set_clipboard_text_windows(text: &str) -> Result<(), io::Error> {
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hWndNewOwner: *mut c_void) -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(uFormat: u32, hMem: *mut c_void) -> *mut c_void;
        fn CloseClipboard() -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut c_void;
        fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hMem: *mut c_void) -> i32;
        fn GlobalFree(hMem: *mut c_void) -> *mut c_void;
    }

    const GMEM_MOVEABLE: u32 = 0x0002;
    const CF_UNICODETEXT: u32 = 13;

    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0); // Null terminator
    let bytes_len = wide.len() * 2;

    let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes_len) };
    if hmem.is_null() {
        return Err(io::Error::last_os_error());
    }

    let ptr = unsafe { GlobalLock(hmem) };
    if ptr.is_null() {
        unsafe {
            GlobalFree(hmem);
        }
        return Err(io::Error::last_os_error());
    }

    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
        GlobalUnlock(hmem);
    }

    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        unsafe {
            GlobalFree(hmem);
        }
        return Err(io::Error::last_os_error());
    }

    unsafe {
        EmptyClipboard();
        let res = SetClipboardData(CF_UNICODETEXT, hmem);
        if res.is_null() {
            let err = io::Error::last_os_error();
            GlobalFree(hmem);
            CloseClipboard();
            return Err(err);
        }
        CloseClipboard();
    }

    Ok(())
}

#[cfg(windows)]
fn get_clipboard_text_windows() -> Result<String, io::Error> {
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hWndNewOwner: *mut c_void) -> i32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn GetClipboardData(uFormat: u32) -> *mut c_void;
        fn CloseClipboard() -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hMem: *mut c_void) -> i32;
        fn GlobalSize(hMem: *mut c_void) -> usize;
    }

    const CF_UNICODETEXT: u32 = 13;

    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "clipboard does not contain UTF-16 text",
            ));
        }

        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        let ptr = unsafe { GlobalLock(handle) };
        if ptr.is_null() {
            return Err(io::Error::last_os_error());
        }

        let byte_len = unsafe { GlobalSize(handle) };
        let unit_len = byte_len / 2;
        let slice = unsafe { std::slice::from_raw_parts(ptr as *const u16, unit_len) };
        let end = slice.iter().position(|unit| *unit == 0).unwrap_or(unit_len);
        let text = String::from_utf16_lossy(&slice[..end]);
        unsafe {
            GlobalUnlock(handle);
        }

        Ok(text)
    })();

    unsafe {
        CloseClipboard();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static CLIPBOARD_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_clipboard_does_not_panic() {
        let _guard = CLIPBOARD_TEST_LOCK.lock().unwrap();
        let original = get_clipboard_text().ok();
        let text = "Identity local-first clipboard test text.";
        let res = set_clipboard_text(text);
        #[cfg(windows)]
        {
            assert!(res.is_ok());
            if let Some(original) = original {
                let _ = set_clipboard_text(&original);
            }
        }
        #[cfg(not(windows))]
        assert!(res.is_err());
    }

    #[test]
    fn test_clipboard_round_trip_text() {
        let _guard = CLIPBOARD_TEST_LOCK.lock().unwrap();
        let original = get_clipboard_text().ok();
        let text = "Identity clipboard read test.";
        let write = set_clipboard_text(text);
        #[cfg(windows)]
        {
            write.unwrap();
            assert_eq!(get_clipboard_text().unwrap(), text);
            if let Some(original) = original {
                let _ = set_clipboard_text(&original);
            }
        }
        #[cfg(not(windows))]
        {
            assert!(write.is_err());
            assert!(get_clipboard_text().is_err());
        }
    }
}
