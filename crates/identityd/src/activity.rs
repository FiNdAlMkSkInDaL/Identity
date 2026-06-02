use crate::transit::{TransitBuffer, TransitError};
use crate::workspace::IdentityPaths;
use std::fmt;
#[cfg(windows)]
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::{sleep, Duration};

pub const DEFAULT_ACTIVITY_POLL_MS: u64 = 1000;
#[cfg(windows)]
const MAX_VISIBLE_TEXT_ENTRIES: usize = 12;
#[cfg(windows)]
const MAX_VISIBLE_TEXT_CHARS: usize = 512;
#[cfg(windows)]
const MAX_WINDOW_TEXT_CHARS: usize = 1024;
#[cfg(windows)]
const WINDOW_TEXT_TIMEOUT_MS: usize = 25;
#[cfg(windows)]
const VT_I4: u16 = 3;
#[cfg(windows)]
const CHILDID_SELF: i32 = 0;
#[cfg(windows)]
const OBJID_CLIENT: u32 = 0xFFFF_FFFCu32;
#[cfg(windows)]
const COINIT_APARTMENTTHREADED: u32 = 0x2;
#[cfg(windows)]
const RPC_E_CHANGED_MODE: i32 = -2147417850;

#[derive(Debug)]
pub enum ActivityError {
    EmptyCapture,
    Transit(TransitError),
    UnsupportedPlatform,
}

impl fmt::Display for ActivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCapture => write!(f, "active window did not expose captureable text"),
            Self::Transit(error) => write!(f, "{error}"),
            Self::UnsupportedPlatform => {
                write!(
                    f,
                    "active window capture is currently implemented only on Windows"
                )
            }
        }
    }
}

impl std::error::Error for ActivityError {}

impl From<TransitError> for ActivityError {
    fn from(value: TransitError) -> Self {
        Self::Transit(value)
    }
}

pub fn capture_active_window_once(paths: &IdentityPaths) -> Result<i64, ActivityError> {
    #[cfg(windows)]
    {
        let snapshot = capture_foreground_window_snapshot()?.ok_or(ActivityError::EmptyCapture)?;
        let content = format_capture_content(&snapshot);
        let buffer = TransitBuffer::open(paths)?;
        return buffer
            .ingest_text("windows-ui:foreground-window", &content)
            .map_err(ActivityError::from);
    }

    #[allow(unreachable_code)]
    Err(ActivityError::UnsupportedPlatform)
}

pub async fn watch_active_window_until_shutdown(
    paths: IdentityPaths,
    poll_interval_ms: u64,
    shutdown: Arc<AtomicBool>,
) -> Result<(), ActivityError> {
    #[cfg(windows)]
    {
        let buffer = TransitBuffer::open(&paths)?;
        let poll_interval = Duration::from_millis(poll_interval_ms.max(100));
        let mut last_snapshot = None;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                return Ok(());
            }

            if let Some(snapshot) = capture_foreground_window_snapshot()? {
                if should_emit_snapshot(last_snapshot.as_ref(), &snapshot) {
                    let id = buffer.ingest_text(
                        "windows-ui:foreground-window",
                        &format_capture_content(&snapshot),
                    )?;
                    println!(
                        "queued active window capture #{id} from {}",
                        snapshot.application
                    );
                    last_snapshot = Some(snapshot);
                }
            }

            sleep(poll_interval).await;
        }
    }

    #[allow(unreachable_code)]
    {
        let _ = paths;
        let _ = poll_interval_ms;
        let _ = shutdown;
        Err(ActivityError::UnsupportedPlatform)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowSnapshot {
    application: String,
    title: String,
    focused_text: Option<String>,
    visible_text: Vec<String>,
}

fn format_capture_content(snapshot: &WindowSnapshot) -> String {
    let mut content = format!(
        "Active application: {application}\nActive window title: {title}",
        application = snapshot.application,
        title = snapshot.title
    );

    if let Some(focused_text) = snapshot.focused_text.as_ref() {
        content.push_str("\nFocused control text: ");
        content.push_str(focused_text);
    }

    if !snapshot.visible_text.is_empty() {
        content.push_str("\nVisible window text:");
        for line in &snapshot.visible_text {
            content.push_str("\n- ");
            content.push_str(line);
        }
    }

    content
}

fn should_emit_snapshot(previous: Option<&WindowSnapshot>, current: &WindowSnapshot) -> bool {
    previous != Some(current)
}

#[cfg(windows)]
fn normalize_capture_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(windows)]
fn prefer_text(primary: String, fallback: String) -> String {
    match (primary.is_empty(), fallback.is_empty()) {
        (true, false) => fallback,
        (false, true) => primary,
        (false, false) if fallback.len() > primary.len() => fallback,
        _ => primary,
    }
}

#[cfg(windows)]
fn capture_foreground_window_snapshot() -> Result<Option<WindowSnapshot>, ActivityError> {
    use std::ffi::c_void;

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;
    type Hwnd = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x1000;

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

    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindowThreadProcessId(hWnd: Hwnd, lpdwProcessId: *mut Dword) -> Dword;
    }

    struct ProcessHandle(Handle);

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return Ok(None);
    }

    let title = read_window_text(hwnd);

    if title.is_empty() {
        return Ok(None);
    }

    let visible_text = collect_visible_window_text(hwnd, &title);
    let focused_text = focused_control_text(hwnd, title.as_str());

    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Ok(Some(WindowSnapshot {
            application: "unknown".to_string(),
            title,
            focused_text,
            visible_text,
        }));
    }

    let _guard = ProcessHandle(process);
    let mut exe = vec![0_u16; 260];
    let mut len = exe.len() as Dword;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, exe.as_mut_ptr(), &mut len) };

    let application = if ok == 0 || len == 0 {
        "unknown".to_string()
    } else {
        let raw = String::from_utf16_lossy(&exe[..len as usize]);
        executable_name(&raw).unwrap_or_else(|| "unknown".to_string())
    };

    Ok(Some(WindowSnapshot {
        application,
        title,
        focused_text,
        visible_text,
    }))
}

#[cfg(windows)]
fn focused_control_text(root_hwnd: *mut std::ffi::c_void, title: &str) -> Option<String> {
    use std::ffi::c_void;

    type Bool = i32;
    type Dword = u32;
    type Hwnd = *mut c_void;

    #[repr(C)]
    struct GuiThreadInfo {
        cb_size: Dword,
        flags: Dword,
        hwnd_active: Hwnd,
        hwnd_focus: Hwnd,
        hwnd_capture: Hwnd,
        hwnd_menu_owner: Hwnd,
        hwnd_move_size: Hwnd,
        hwnd_caret: Hwnd,
        caret_left: i32,
        caret_top: i32,
        caret_right: i32,
        caret_bottom: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetGUIThreadInfo(idThread: Dword, pgui: *mut GuiThreadInfo) -> Bool;
        fn IsChild(hWndParent: Hwnd, hWnd: Hwnd) -> Bool;
    }

    let mut gui = GuiThreadInfo {
        cb_size: std::mem::size_of::<GuiThreadInfo>() as Dword,
        flags: 0,
        hwnd_active: std::ptr::null_mut(),
        hwnd_focus: std::ptr::null_mut(),
        hwnd_capture: std::ptr::null_mut(),
        hwnd_menu_owner: std::ptr::null_mut(),
        hwnd_move_size: std::ptr::null_mut(),
        hwnd_caret: std::ptr::null_mut(),
        caret_left: 0,
        caret_top: 0,
        caret_right: 0,
        caret_bottom: 0,
    };

    if unsafe { GetGUIThreadInfo(0, &mut gui) } == 0 || gui.hwnd_focus.is_null() {
        return None;
    }

    if gui.hwnd_focus != root_hwnd && unsafe { IsChild(root_hwnd, gui.hwnd_focus) } == 0 {
        return None;
    }

    let focused_text = prefer_text(
        read_window_text(gui.hwnd_focus),
        read_accessible_text(gui.hwnd_focus),
    );
    if focused_text.is_empty() || focused_text == title {
        None
    } else {
        Some(focused_text)
    }
}

#[cfg(windows)]
fn read_accessible_text(hwnd: *mut std::ffi::c_void) -> String {
    use std::ffi::c_void;

    type Hresult = i32;
    type Bstr = *mut u16;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    union VariantData {
        l_val: i32,
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct Variant {
        vt: u16,
        w_reserved1: u16,
        w_reserved2: u16,
        w_reserved3: u16,
        data: VariantData,
    }

    #[repr(C)]
    struct IAccessible {
        lp_vtbl: *const IAccessibleVtbl,
    }

    #[repr(C)]
    struct IAccessibleVtbl {
        query_interface: usize,
        add_ref: unsafe extern "system" fn(*mut IAccessible) -> u32,
        release: unsafe extern "system" fn(*mut IAccessible) -> u32,
        get_type_info_count: usize,
        get_type_info: usize,
        get_ids_of_names: usize,
        invoke: usize,
        get_acc_parent: usize,
        get_acc_child_count: usize,
        get_acc_child: usize,
        get_acc_name: unsafe extern "system" fn(*mut IAccessible, Variant, *mut Bstr) -> Hresult,
        get_acc_value: unsafe extern "system" fn(*mut IAccessible, Variant, *mut Bstr) -> Hresult,
    }

    const IID_IACCESSIBLE: Guid = Guid {
        data1: 0x6187_36E0,
        data2: 0x3C3D,
        data3: 0x11CF,
        data4: [0x81, 0x0C, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    };

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, coinit: u32) -> Hresult;
        fn CoUninitialize();
    }

    #[link(name = "oleacc")]
    extern "system" {
        fn AccessibleObjectFromWindow(
            hwnd: *mut c_void,
            object_id: u32,
            riid: *const Guid,
            object: *mut *mut c_void,
        ) -> Hresult;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysFreeString(bstr: Bstr);
        fn SysStringLen(bstr: Bstr) -> u32;
    }

    struct ComInitGuard(bool);

    impl Drop for ComInitGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    struct AccessibleGuard(*mut IAccessible);

    impl Drop for AccessibleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    ((*(*self.0).lp_vtbl).release)(self.0);
                }
            }
        }
    }

    fn variant_self() -> Variant {
        Variant {
            vt: VT_I4,
            w_reserved1: 0,
            w_reserved2: 0,
            w_reserved3: 0,
            data: VariantData {
                l_val: CHILDID_SELF,
            },
        }
    }

    fn take_bstr_text(value: Bstr) -> String {
        if value.is_null() {
            return String::new();
        }

        let len = unsafe { SysStringLen(value) as usize };
        let text = normalize_capture_text(&String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(value, len)
        }));

        unsafe {
            SysFreeString(value);
        }

        text
    }

    let init = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let _com_guard = if init >= 0 {
        ComInitGuard(true)
    } else if init == RPC_E_CHANGED_MODE {
        ComInitGuard(false)
    } else {
        return String::new();
    };

    let mut raw_object = std::ptr::null_mut();
    let accessible_result = unsafe {
        AccessibleObjectFromWindow(hwnd, OBJID_CLIENT, &IID_IACCESSIBLE, &mut raw_object)
    };

    if accessible_result < 0 || raw_object.is_null() {
        return String::new();
    }

    let accessible = AccessibleGuard(raw_object.cast::<IAccessible>());
    let self_variant = variant_self();

    let mut acc_name = std::ptr::null_mut();
    let name_result = unsafe {
        ((*(*accessible.0).lp_vtbl).get_acc_name)(accessible.0, self_variant, &mut acc_name)
    };
    let name = if name_result >= 0 {
        take_bstr_text(acc_name)
    } else {
        String::new()
    };

    let mut acc_value = std::ptr::null_mut();
    let value_result = unsafe {
        ((*(*accessible.0).lp_vtbl).get_acc_value)(accessible.0, self_variant, &mut acc_value)
    };
    let value = if value_result >= 0 {
        take_bstr_text(acc_value)
    } else {
        String::new()
    };

    prefer_text(name, value)
}

#[cfg(windows)]
fn read_window_text(hwnd: *mut std::ffi::c_void) -> String {
    let primary = read_window_text_via_caption(hwnd);
    let fallback = read_window_text_via_message(hwnd);

    prefer_text(primary, fallback)
}

#[cfg(windows)]
fn read_window_text_via_caption(hwnd: *mut std::ffi::c_void) -> String {
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowTextLengthW(hWnd: *mut std::ffi::c_void) -> i32;
        fn GetWindowTextW(hWnd: *mut std::ffi::c_void, lpString: *mut u16, nMaxCount: i32) -> i32;
    }

    let title_len = unsafe { GetWindowTextLengthW(hwnd) };
    if title_len <= 0 {
        return String::new();
    }

    let mut title_buffer = vec![0_u16; title_len as usize + 1];
    let title_written =
        unsafe { GetWindowTextW(hwnd, title_buffer.as_mut_ptr(), title_buffer.len() as i32) };

    normalize_capture_text(&String::from_utf16_lossy(
        &title_buffer[..title_written.max(0) as usize],
    ))
}

#[cfg(windows)]
fn read_window_text_via_message(hwnd: *mut std::ffi::c_void) -> String {
    use std::ffi::c_void;

    type Lresult = isize;
    type Uint = u32;
    type Wparam = usize;
    type Lparam = isize;
    type UlongPtr = usize;
    type Hwnd = *mut c_void;

    const WM_GETTEXT: Uint = 0x000D;
    const WM_GETTEXTLENGTH: Uint = 0x000E;
    const SMTO_ABORTIFHUNG: Uint = 0x0002;
    const SMTO_BLOCK: Uint = 0x0001;

    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            hWnd: Hwnd,
            msg: Uint,
            w_param: Wparam,
            l_param: Lparam,
            fu_flags: Uint,
            u_timeout: Uint,
            lpdw_result: *mut UlongPtr,
        ) -> Lresult;
    }

    let mut length_result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXTLENGTH,
            0,
            0,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            WINDOW_TEXT_TIMEOUT_MS as u32,
            &mut length_result,
        )
    };

    if sent == 0 || length_result == 0 {
        return String::new();
    }

    let text_len = length_result.min(MAX_WINDOW_TEXT_CHARS.saturating_sub(1));
    if text_len == 0 {
        return String::new();
    }

    let mut buffer = vec![0u16; text_len + 1];
    let mut copied = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            WINDOW_TEXT_TIMEOUT_MS as u32,
            &mut copied,
        )
    };

    if sent == 0 {
        return String::new();
    }

    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    normalize_capture_text(&String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(windows)]
fn collect_visible_window_text(hwnd: *mut std::ffi::c_void, title: &str) -> Vec<String> {
    use std::ffi::c_void;

    type Bool = i32;
    type Hwnd = *mut c_void;
    type Lparam = isize;

    struct ChildTextCollector {
        skip: String,
        entries: Vec<String>,
        total_chars: usize,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumChildWindows(
            hWndParent: Hwnd,
            lpEnumFunc: extern "system" fn(Hwnd, Lparam) -> Bool,
            lParam: Lparam,
        ) -> Bool;
        fn IsWindowVisible(hWnd: Hwnd) -> Bool;
    }

    extern "system" fn collect_child_text(hwnd: Hwnd, lparam: Lparam) -> Bool {
        let collector = unsafe { &mut *(lparam as *mut ChildTextCollector) };

        #[link(name = "user32")]
        extern "system" {
            fn IsWindowVisible(hWnd: Hwnd) -> Bool;
        }

        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }

        let text = read_window_text(hwnd);
        if text.is_empty()
            || text == collector.skip
            || collector.entries.iter().any(|entry| entry == &text)
        {
            return 1;
        }

        if collector.entries.len() >= MAX_VISIBLE_TEXT_ENTRIES
            || collector.total_chars + text.len() > MAX_VISIBLE_TEXT_CHARS
        {
            return 0;
        }

        collector.total_chars += text.len();
        collector.entries.push(text);
        1
    }

    let mut collector = ChildTextCollector {
        skip: title.to_string(),
        entries: Vec::new(),
        total_chars: 0,
    };

    if unsafe { IsWindowVisible(hwnd) } != 0 {
        let root_text = read_window_text(hwnd);
        if !root_text.is_empty() && root_text != collector.skip {
            collector.total_chars += root_text.len().min(MAX_VISIBLE_TEXT_CHARS);
            collector.entries.push(root_text);
        }
    }

    unsafe {
        EnumChildWindows(
            hwnd,
            collect_child_text,
            (&mut collector as *mut ChildTextCollector) as Lparam,
        );
    }

    collector.entries
}

#[cfg(windows)]
fn executable_name(path: &str) -> Option<String> {
    let file_name = Path::new(path).file_name()?.to_str()?.trim();
    if file_name.is_empty() {
        None
    } else {
        Some(file_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{format_capture_content, prefer_text, should_emit_snapshot, WindowSnapshot};

    #[cfg(windows)]
    use super::executable_name;

    #[test]
    fn formats_window_capture_as_plain_local_context() {
        let content = format_capture_content(&WindowSnapshot {
            application: "Code.exe".to_string(),
            title: "Identity - README.md".to_string(),
            focused_text: Some("Search files".to_string()),
            visible_text: vec!["Identity local-first notes".to_string()],
        });

        assert!(content.contains("Active application: Code.exe"));
        assert!(content.contains("Active window title: Identity - README.md"));
        assert!(content.contains("Focused control text: Search files"));
        assert!(content.contains("Visible window text:"));
        assert!(content.contains("Identity local-first notes"));
    }

    #[test]
    fn emits_only_meaningful_foreground_changes() {
        let first = WindowSnapshot {
            application: "Code.exe".to_string(),
            title: "Identity - README.md".to_string(),
            focused_text: Some("README.md".to_string()),
            visible_text: vec!["Identity".to_string()],
        };
        let second = WindowSnapshot {
            application: "Code.exe".to_string(),
            title: "Identity - README.md".to_string(),
            focused_text: Some("README.md".to_string()),
            visible_text: vec!["Identity".to_string()],
        };
        let third = WindowSnapshot {
            application: "Code.exe".to_string(),
            title: "Identity - Cargo.toml".to_string(),
            focused_text: Some("Cargo.toml".to_string()),
            visible_text: vec!["Cargo.toml".to_string()],
        };

        assert!(should_emit_snapshot(None, &first));
        assert!(!should_emit_snapshot(Some(&first), &second));
        assert!(should_emit_snapshot(Some(&first), &third));
    }

    #[test]
    fn prefers_richer_fallback_text_when_available() {
        assert_eq!(
            prefer_text("Save".to_string(), "Save draft to workspace".to_string()),
            "Save draft to workspace"
        );
        assert_eq!(prefer_text("Open".to_string(), String::new()), "Open");
    }

    #[cfg(windows)]
    #[test]
    fn accessibility_variant_targets_self_child() {
        assert_eq!(super::VT_I4, 3);
        assert_eq!(super::CHILDID_SELF, 0);
    }

    #[cfg(windows)]
    #[test]
    fn extracts_executable_basename_from_windows_path() {
        assert_eq!(
            executable_name(r"C:\Program Files\Microsoft VS Code\Code.exe"),
            Some("Code.exe".to_string())
        );
    }
}
