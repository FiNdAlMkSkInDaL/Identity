use crate::context_builder::build_identity_context;
use crate::context_snapshot::capture_context_snapshot;
use crate::project_profile::{find_matching_profile, load_profiles};
use crate::workspace::IdentityPaths;
use crate::clipboard::set_clipboard_text;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct HotkeyHandle {
    #[cfg(windows)]
    thread_id: u32,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for HotkeyHandle {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            #[link(name = "user32")]
            extern "system" {
                fn PostThreadMessageW(idThread: u32, Msg: u32, wParam: usize, lParam: isize) -> i32;
            }
            unsafe {
                PostThreadMessageW(self.thread_id, 0x0012, 0, 0); // WM_QUIT = 0x0012
            }
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Parse a hotkey combo string (e.g. "ctrl+alt+i") into Win32 modifiers and key code.
pub fn parse_hotkey_combo(combo: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() {
        return Err("empty hotkey combo".to_string());
    }

    let mut modifiers = 0x4000; // MOD_NOREPEAT = 0x4000 by default
    let key_str = parts.last().ok_or_else(|| "missing main key".to_string())?.trim();

    for &part in parts.iter().take(parts.len() - 1) {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= 0x0002, // MOD_CONTROL
            "alt" => modifiers |= 0x0001,             // MOD_ALT
            "shift" => modifiers |= 0x0004,           // MOD_SHIFT
            "win" | "super" | "meta" => modifiers |= 0x0008, // MOD_WIN
            _ => return Err(format!("unknown hotkey modifier: {}", part)),
        }
    }

    let vk = if key_str.len() == 1 {
        let ch = key_str.chars().next().unwrap().to_ascii_uppercase();
        if ch.is_ascii_alphanumeric() {
            ch as u32
        } else {
            return Err(format!("unsupported hotkey key character: {}", ch));
        }
    } else {
        match key_str.to_ascii_uppercase().as_str() {
            "F1" => 0x70,
            "F2" => 0x71,
            "F3" => 0x72,
            "F4" => 0x73,
            "F5" => 0x74,
            "F6" => 0x75,
            "F7" => 0x76,
            "F8" => 0x77,
            "F9" => 0x78,
            "F10" => 0x79,
            "F11" => 0x7A,
            "F12" => 0x7B,
            _ => return Err(format!("unknown hotkey key: {}", key_str)),
        }
    };

    Ok((modifiers, vk))
}

/// Spawns a background thread that registers the specified hotkey combo
/// and listens for it using a native Win32 message loop.
pub fn start_hotkey_listener(
    paths: IdentityPaths,
    combo: &str,
    paste_on_hotkey: bool,
    shutdown: Arc<AtomicBool>,
) -> Result<HotkeyHandle, String> {
    #[cfg(not(windows))]
    {
        let _ = (paths, combo, paste_on_hotkey, shutdown);
        Err("global hotkeys are only supported on Windows".to_string())
    }

    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::sync::mpsc;

        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }

        #[repr(C)]
        struct Msg {
            hwnd: *mut c_void,
            message: u32,
            w_param: usize,
            l_param: isize,
            time: u32,
            pt: Point,
        }

        #[link(name = "user32")]
        extern "system" {
            fn RegisterHotKey(hWnd: *mut c_void, id: i32, fsModifiers: u32, vk: u32) -> i32;
            fn UnregisterHotKey(hWnd: *mut c_void, id: i32) -> i32;
            fn GetMessageW(lpMsg: *mut Msg, hWnd: *mut c_void, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
            fn keybd_event(bVk: u8, bScan: u8, dwFlags: u32, dwExtraInfo: usize);
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThreadId() -> u32;
        }

        let (mod_flags, vk) = parse_hotkey_combo(combo)?;
        let combo_str = combo.to_string();
        let (tx, rx) = mpsc::channel();

        let paths_clone = paths.clone();
        let shutdown_clone = shutdown.clone();

        let join_handle = thread::spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            const HOTKEY_ID: i32 = 1279;

            let ok = unsafe { RegisterHotKey(std::ptr::null_mut(), HOTKEY_ID, mod_flags, vk) };
            if ok == 0 {
                let _ = tx.send(Err(format!(
                    "failed to register hotkey combo '{}' (already in use or invalid)",
                    combo_str
                )));
                return;
            }

            let _ = tx.send(Ok(thread_id));

            let mut msg = Msg {
                hwnd: std::ptr::null_mut(),
                message: 0,
                w_param: 0,
                l_param: 0,
                time: 0,
                pt: Point { x: 0, y: 0 },
            };

            let mut last_trigger = Instant::now() - Duration::from_secs(5);
            const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }

                if msg.message == 0x0312 { // WM_HOTKEY
                    let now = Instant::now();
                    if now.duration_since(last_trigger) >= DEBOUNCE_DURATION {
                        last_trigger = now;

                        // Trigger context generation
                        let snapshot = capture_context_snapshot().unwrap_or_default();
                        if let Ok(profiles) = load_profiles(&paths_clone) {
                            let matched = find_matching_profile(&profiles, &snapshot);
                            // Query up to 3 relevant memory records
                            if let Ok(context) = build_identity_context(&paths_clone, &snapshot, matched.as_ref(), 3) {
                                let block = context.to_context_block();
                                if set_clipboard_text(&block).is_ok() {
                                    println!("hotkey triggered: compiled context copied to clipboard");
                                    if paste_on_hotkey {
                                        thread::sleep(Duration::from_millis(50));
                                        unsafe {
                                            // Simulate Ctrl+V key event sequence
                                            keybd_event(0x11, 0, 0, 0); // Ctrl down
                                            keybd_event(0x56, 0, 0, 0); // V down
                                            keybd_event(0x56, 0, 0x0002, 0); // V up
                                            keybd_event(0x11, 0, 0x0002, 0); // Ctrl up
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
            }
        });

        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(HotkeyHandle {
                thread_id,
                join_handle: Some(join_handle),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("hotkey initialization thread disconnected".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_combo() {
        let (mods, vk) = parse_hotkey_combo("ctrl+alt+i").unwrap();
        assert_eq!(mods & 0x0002, 0x0002); // MOD_CONTROL
        assert_eq!(mods & 0x0001, 0x0001); // MOD_ALT
        assert_eq!(vk, 0x49);             // 'I'

        let (mods2, vk2) = parse_hotkey_combo("win+shift+f1").unwrap();
        assert_eq!(mods2 & 0x0008, 0x0008); // MOD_WIN
        assert_eq!(mods2 & 0x0004, 0x0004); // MOD_SHIFT
        assert_eq!(vk2, 0x70);             // F1

        assert!(parse_hotkey_combo("invalid+modifier+k").is_err());
    }
}
