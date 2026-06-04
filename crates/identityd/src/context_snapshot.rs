/// Phase 2 — on-demand context snapshot.
///
/// Reads the current foreground window metadata (process name, title, focused-control text)
/// without queuing anything in the transit buffer. This is the zero-latency read path used
/// by the hotkey pipeline: press hotkey → read window → build context → write clipboard.
use std::fmt;

/// Metadata about the currently active foreground window.
#[derive(Debug, Clone, Default)]
pub struct ContextSnapshot {
    /// Short executable basename, e.g. `"chrome"`, `"code"`, `"gemini"`.
    pub process_name: String,
    /// Window title text.
    pub window_title: String,
    /// Text from the focused UI control, if any (UIA / MSAA / WM_GETTEXT chain).
    pub focused_text: Option<String>,
}

#[derive(Debug)]
pub enum SnapshotError {
    UnsupportedPlatform,
    /// No foreground window was visible (e.g., desktop has focus).
    NoForegroundWindow,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "active window snapshot is only supported on Windows"),
            Self::NoForegroundWindow => write!(f, "no foreground window is currently active"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Capture the current foreground window without touching the transit buffer.
pub fn capture_context_snapshot() -> Result<ContextSnapshot, SnapshotError> {
    #[cfg(windows)]
    {
        return capture_context_snapshot_windows();
    }
    #[allow(unreachable_code)]
    Err(SnapshotError::UnsupportedPlatform)
}

#[cfg(windows)]
fn capture_context_snapshot_windows() -> Result<ContextSnapshot, SnapshotError> {
    use std::ffi::c_void;
    use std::path::Path;

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;
    type Hwnd = *mut c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x1000;
    const VT_I4: u16 = 3;
    const CHILDID_SELF: i32 = 0;
    const OBJID_CLIENT: u32 = 0xFFFF_FFFCu32;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const RPC_E_CHANGED_MODE: i32 = -2147417850;
    const MAX_WINDOW_TEXT_CHARS: usize = 1024;
    const WINDOW_TEXT_TIMEOUT_MS: usize = 25;

    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindowThreadProcessId(hWnd: Hwnd, lpdwProcessId: *mut Dword) -> Dword;
        fn SendMessageTimeoutW(
            hWnd: Hwnd,
            Msg: u32,
            wParam: usize,
            lParam: isize,
            fuFlags: u32,
            uTimeout: u32,
            lpdwResult: *mut usize,
        ) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(dwDesiredAccess: Dword, bInheritHandle: Bool, dwProcessId: Dword) -> Handle;
        fn QueryFullProcessImageNameW(
            hProcess: Handle,
            dwFlags: Dword,
            lpExeName: *mut u16,
            lpdwSize: *mut Dword,
        ) -> Bool;
        fn CloseHandle(hObject: Handle) -> Bool;
    }

    struct ProcessHandle(Handle);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0); }
        }
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return Err(SnapshotError::NoForegroundWindow);
    }

    // Read window title via WM_GETTEXT with timeout to avoid hangs.
    let title = {
        const WM_GETTEXT: u32 = 0x000D;
        let mut buf: Vec<u16> = vec![0u16; MAX_WINDOW_TEXT_CHARS + 1];
        let mut result: usize = 0;
        unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETTEXT,
                MAX_WINDOW_TEXT_CHARS,
                buf.as_mut_ptr() as isize,
                0x0002, // SMTO_ABORTIFHUNG
                WINDOW_TEXT_TIMEOUT_MS as u32,
                &mut result,
            );
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim().to_string()
    };

    if title.is_empty() {
        return Err(SnapshotError::NoForegroundWindow);
    }

    // Resolve process name from executable path.
    let mut process_id: Dword = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id); }

    let process_name = if process_id != 0 {
        let proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if !proc.is_null() {
            let guard = ProcessHandle(proc);
            let mut buf: Vec<u16> = vec![0u16; 260];
            let mut size: Dword = buf.len() as Dword;
            let ok = unsafe {
                QueryFullProcessImageNameW(guard.0, 0, buf.as_mut_ptr(), &mut size)
            };
            if ok != 0 {
                let path = String::from_utf16_lossy(&buf[..size as usize]);
                Path::new(&path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
                    .to_string()
            } else {
                "unknown".to_string()
            }
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    // Focused-control text via UIA / MSAA / WM_GETTEXT chain.
    let focused_text = read_focused_text(hwnd, &title, VT_I4, CHILDID_SELF, OBJID_CLIENT,
                                        COINIT_APARTMENTTHREADED, RPC_E_CHANGED_MODE,
                                        MAX_WINDOW_TEXT_CHARS, WINDOW_TEXT_TIMEOUT_MS);

    Ok(ContextSnapshot {
        process_name,
        window_title: title,
        focused_text,
    })
}

/// Attempt to read the focused control's text via UIA, then MSAA, then WM_GETTEXT.
/// Returns `None` if no meaningful text is found or all methods fail.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn read_focused_text(
    hwnd: *mut std::ffi::c_void,
    window_title: &str,
    vt_i4: u16,
    childid_self: i32,
    objid_client: u32,
    coinit_apartmentthreaded: u32,
    rpc_e_changed_mode: i32,
    max_chars: usize,
    timeout_ms: usize,
) -> Option<String> {
    let uia_text = try_uia_focused_text(coinit_apartmentthreaded, rpc_e_changed_mode);
    let msaa_text = try_msaa_focused_text(hwnd, vt_i4, childid_self, objid_client);
    let raw_text = try_wm_gettext(hwnd, max_chars, timeout_ms);

    // Pick the richest, non-empty, non-title-duplicate result.
    for text in [uia_text, msaa_text, raw_text].into_iter().flatten() {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() && trimmed != window_title {
            return Some(trimmed);
        }
    }
    None
}

#[cfg(windows)]
fn try_uia_focused_text(coinit_apartmentthreaded: u32, rpc_e_changed_mode: i32) -> Option<String> {
    use std::ffi::c_void;

    // UIA COM GUIDs and interface definitions (minimal subset).
    #[repr(C)]
    struct Guid {
        data1: u32, data2: u16, data3: u16, data4: [u8; 8],
    }

    // IUnknown vtable stubs — just enough to call Release.
    type HResult = i32;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> HResult;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const Guid,
            pUnkOuter: *mut c_void,
            dwClsContext: u32,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> HResult;
    }

    // IUIAutomation CLSID / IID (from Windows SDK).
    let clsid_uia_automation = Guid {
        data1: 0xff48dba4, data2: 0x60ef, data3: 0x4201,
        data4: [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
    };
    let iid_iuia = Guid {
        data1: 0x30cbe57d, data2: 0xd9d0, data3: 0x452a,
        data4: [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
    };

    unsafe {
        let hr = CoInitializeEx(std::ptr::null_mut(), coinit_apartmentthreaded);
        if hr < 0 && hr != rpc_e_changed_mode {
            return None;
        }
        let coinit_ok = hr >= 0;

        let mut punk: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &clsid_uia_automation,
            std::ptr::null_mut(),
            1, // CLSCTX_INPROC_SERVER
            &iid_iuia,
            &mut punk,
        );
        if hr < 0 || punk.is_null() {
            if coinit_ok { CoUninitialize(); }
            return None;
        }

        // IUIAutomation::GetFocusedElement is at vtable slot 8 (0-indexed).
        // Vtable: [QueryInterface, AddRef, Release, ..., GetFocusedElement=8]
        let vtable = *(punk as *mut *mut *mut usize);
        type GetFocusedElementFn = unsafe extern "system" fn(
            this: *mut c_void,
            element: *mut *mut c_void,
        ) -> HResult;
        let get_focused: GetFocusedElementFn = std::mem::transmute(*vtable.add(8));

        let mut elem: *mut c_void = std::ptr::null_mut();
        let hr = get_focused(punk, &mut elem);

        // Release IUIAutomation.
        let release_uia: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*vtable.add(2));
        release_uia(punk);

        if hr < 0 || elem.is_null() {
            if coinit_ok { CoUninitialize(); }
            return None;
        }

        // IUIAutomationElement — read CurrentValue (slot 19) and CurrentName (slot 13).
        let elem_vtable = *(elem as *mut *mut *mut usize);

        type GetBstrFn = unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HResult;
        let get_value: GetBstrFn = std::mem::transmute(*elem_vtable.add(19));
        let get_name: GetBstrFn = std::mem::transmute(*elem_vtable.add(13));

        #[link(name = "oleaut32")]
        extern "system" {
            fn SysFreeString(bstr: *mut u16);
            fn SysStringLen(bstr: *mut u16) -> u32;
        }

        let mut bstr: *mut u16 = std::ptr::null_mut();
        let value_text = if get_value(elem, &mut bstr) >= 0 && !bstr.is_null() {
            let len = SysStringLen(bstr) as usize;
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(bstr, len)).to_string();
            SysFreeString(bstr);
            if text.is_empty() { None } else { Some(text) }
        } else { None };

        let name_text = if value_text.is_none() {
            let mut nbstr: *mut u16 = std::ptr::null_mut();
            if get_name(elem, &mut nbstr) >= 0 && !nbstr.is_null() {
                let len = SysStringLen(nbstr) as usize;
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(nbstr, len)).to_string();
                SysFreeString(nbstr);
                if text.is_empty() { None } else { Some(text) }
            } else { None }
        } else { None };

        // Release IUIAutomationElement.
        let release_elem: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*elem_vtable.add(2));
        release_elem(elem);

        if coinit_ok { CoUninitialize(); }
        value_text.or(name_text)
    }
}

#[cfg(windows)]
#[allow(clashing_extern_declarations)]
fn try_msaa_focused_text(
    hwnd: *mut std::ffi::c_void,
    vt_i4: u16,
    childid_self: i32,
    objid_client: u32,
) -> Option<String> {
    use std::ffi::c_void;
    type HResult = i32;

    #[repr(C)]
    struct Variant {
        vt: u16,
        r1: u16, r2: u16, r3: u16,
        val: i64,
    }

    #[link(name = "oleacc")]
    extern "system" {
        fn AccessibleObjectFromWindow(
            hwnd: *mut c_void,
            dwId: u32,
            riid: *const [u8; 16],
            ppvObject: *mut *mut c_void,
        ) -> HResult;
    }

    const IID_IACCESSIBLE: [u8; 16] = [
        0x18, 0xc3, 0x5b, 0x61, 0x90, 0x44, 0xcf, 0x11,
        0xa8, 0x38, 0x00, 0xdd, 0x01, 0x06, 0x62, 0x25,
    ];

    let mut acc: *mut c_void = std::ptr::null_mut();
    unsafe {
        let hr = AccessibleObjectFromWindow(hwnd, objid_client, &IID_IACCESSIBLE, &mut acc);
        if hr < 0 || acc.is_null() {
            return None;
        }

        let vtable = *(acc as *mut *mut *mut usize);
        #[link(name = "oleaut32")]
        extern "system" {
            fn SysFreeString(bstr: *mut u16);
            fn SysStringLen(bstr: *mut u16) -> u32;
        }

        type GetAccNameFn = unsafe extern "system" fn(
            *mut c_void,
            Variant,
            *mut *mut u16,
        ) -> HResult;
        type GetAccValueFn = unsafe extern "system" fn(
            *mut c_void,
            Variant,
            *mut *mut u16,
        ) -> HResult;

        let self_variant = Variant { vt: vt_i4, r1: 0, r2: 0, r3: 0, val: childid_self as i64 };

        let get_value: GetAccValueFn = std::mem::transmute(*vtable.add(10));
        let get_name: GetAccNameFn = std::mem::transmute(*vtable.add(7));

        let mut bstr: *mut u16 = std::ptr::null_mut();
        let value_text = if get_value(acc, self_variant, &mut bstr) >= 0 && !bstr.is_null() {
            let len = SysStringLen(bstr) as usize;
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(bstr, len)).to_string();
            SysFreeString(bstr);
            if text.is_empty() { None } else { Some(text) }
        } else { None };

        let name_text = if value_text.is_none() {
            let nv = Variant { vt: vt_i4, r1: 0, r2: 0, r3: 0, val: childid_self as i64 };
            let mut nbstr: *mut u16 = std::ptr::null_mut();
            if get_name(acc, nv, &mut nbstr) >= 0 && !nbstr.is_null() {
                let len = SysStringLen(nbstr) as usize;
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(nbstr, len)).to_string();
                SysFreeString(nbstr);
                if text.is_empty() { None } else { Some(text) }
            } else { None }
        } else { None };

        let release: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*vtable.add(2));
        release(acc);

        value_text.or(name_text)
    }
}

#[cfg(windows)]
fn try_wm_gettext(
    hwnd: *mut std::ffi::c_void,
    max_chars: usize,
    timeout_ms: usize,
) -> Option<String> {
    const WM_GETTEXT: u32 = 0x000D;
    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            hWnd: *mut std::ffi::c_void,
            Msg: u32,
            wParam: usize,
            lParam: isize,
            fuFlags: u32,
            uTimeout: u32,
            lpdwResult: *mut usize,
        ) -> isize;
    }

    let mut buf: Vec<u16> = vec![0u16; max_chars + 1];
    let mut result: usize = 0;
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            max_chars,
            buf.as_mut_ptr() as isize,
            0x0002,
            timeout_ms as u32,
            &mut result,
        );
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let text = String::from_utf16_lossy(&buf[..len]).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    use super::{capture_context_snapshot, ContextSnapshot};

    #[test]
    fn snapshot_has_default_state() {
        let snap = ContextSnapshot::default();
        assert!(snap.process_name.is_empty());
        assert!(snap.window_title.is_empty());
        assert!(snap.focused_text.is_none());
    }

    #[test]
    fn capture_does_not_panic() {
        // We can't assert on content in a headless test environment,
        // but capture must not panic or unwind.
        let _ = capture_context_snapshot();
    }
}
