use crate::clipboard::set_clipboard_text;
use crate::context_builder::build_identity_context;
use crate::context_snapshot::capture_context_snapshot;
use crate::project_profile::{find_matching_profile, load_profiles};
use crate::workspace::IdentityPaths;
use std::io::Write;
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
                fn PostThreadMessageW(idThread: u32, Msg: u32, wParam: usize, lParam: isize)
                    -> i32;
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

/// Parse a hotkey combo string (e.g. "ctrl+shift+i") into Win32 modifiers and key code.
pub fn parse_hotkey_combo(combo: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() {
        return Err("empty hotkey combo".to_string());
    }

    let mut modifiers = 0x4000; // MOD_NOREPEAT = 0x4000 by default
    let key_str = parts
        .last()
        .ok_or_else(|| "missing main key".to_string())?
        .trim();

    for &part in parts.iter().take(parts.len() - 1) {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= 0x0002, // MOD_CONTROL
            "alt" => modifiers |= 0x0001,              // MOD_ALT
            "shift" => modifiers |= 0x0004,            // MOD_SHIFT
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
            "SPACE" => 0x20,
            "TAB" => 0x09,
            "ESC" | "ESCAPE" => 0x1B,
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
            l_private: u32,
        }

        #[link(name = "user32")]
        extern "system" {
            fn RegisterHotKey(hWnd: *mut c_void, id: i32, fsModifiers: u32, vk: u32) -> i32;
            fn UnregisterHotKey(hWnd: *mut c_void, id: i32) -> i32;
            fn PeekMessageW(
                lpMsg: *mut Msg,
                hWnd: *mut c_void,
                wMsgFilterMin: u32,
                wMsgFilterMax: u32,
                wRemoveMsg: u32,
            ) -> i32;
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
                l_private: 0,
            };

            let mut last_trigger = Instant::now() - Duration::from_secs(5);
            let mut was_pressed = false;
            const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);
            const POLL_INTERVAL: Duration = Duration::from_millis(25);
            const WM_HOTKEY: u32 = 0x0312;
            const WM_QUIT: u32 = 0x0012;
            const PM_REMOVE: u32 = 0x0001;

            while !shutdown_clone.load(Ordering::Relaxed) {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }

                let mut message_triggered = false;
                while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } > 0 {
                    if msg.message == WM_QUIT {
                        unsafe {
                            UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
                        }
                        return;
                    }
                    if msg.message == WM_HOTKEY {
                        message_triggered = true;
                    }
                }

                let pressed = unsafe { hotkey_pressed(mod_flags, vk) };
                let polling_triggered = pressed && !was_pressed;
                was_pressed = pressed;

                if message_triggered || polling_triggered {
                    let now = Instant::now();
                    if now.duration_since(last_trigger) >= DEBOUNCE_DURATION {
                        last_trigger = now;
                        trigger_context_copy(&paths_clone, paste_on_hotkey);
                    }
                }

                thread::sleep(POLL_INTERVAL);
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

#[cfg(windows)]
unsafe fn hotkey_pressed(mod_flags: u32, vk: u32) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(vKey: i32) -> i16;
    }

    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_WIN: u32 = 0x0008;

    fn is_down(state: i16) -> bool {
        (state as u16 & 0x8000) != 0
    }

    let main_down = is_down(GetAsyncKeyState(vk as i32));
    let ctrl_ok = (mod_flags & MOD_CONTROL == 0) || is_down(GetAsyncKeyState(0x11));
    let alt_ok = (mod_flags & MOD_ALT == 0) || is_down(GetAsyncKeyState(0x12));
    let shift_ok = (mod_flags & MOD_SHIFT == 0) || is_down(GetAsyncKeyState(0x10));
    let win_ok = (mod_flags & MOD_WIN == 0)
        || is_down(GetAsyncKeyState(0x5B))
        || is_down(GetAsyncKeyState(0x5C));

    main_down && ctrl_ok && alt_ok && shift_ok && win_ok
}

#[cfg(windows)]
fn trigger_context_copy(paths: &IdentityPaths, paste_on_hotkey: bool) {
    let snapshot = capture_context_snapshot().unwrap_or_default();
    let profiles = match load_profiles(paths) {
        Ok(profiles) => profiles,
        Err(error) => {
            log_error(&format!(
                "hotkey trigger failed to load project profiles: {error}"
            ));
            return;
        }
    };
    let matched = find_matching_profile(&profiles, &snapshot);
    let context = match build_identity_context(paths, &snapshot, matched.as_ref(), 3) {
        Ok(context) => context,
        Err(error) => {
            log_error(&format!("hotkey trigger failed to build context: {error}"));
            return;
        }
    };
    let block = context.to_context_block();
    if let Err(error) = set_clipboard_text(&block) {
        log_error(&format!(
            "hotkey trigger failed to write clipboard: {error}"
        ));
        return;
    }

    log_info("hotkey triggered: compiled context copied to clipboard");
    if paste_on_hotkey {
        thread::sleep(Duration::from_millis(50));
        unsafe {
            keybd_event(0x11, 0, 0, 0); // Ctrl down
            keybd_event(0x56, 0, 0, 0); // V down
            keybd_event(0x56, 0, 0x0002, 0); // V up
            keybd_event(0x11, 0, 0x0002, 0); // Ctrl up
        }
    }
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn keybd_event(bVk: u8, bScan: u8, dwFlags: u32, dwExtraInfo: usize);
}

#[cfg(windows)]
fn log_info(message: &str) {
    let _ = writeln!(std::io::stdout(), "{message}");
}

#[cfg(windows)]
fn log_error(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_combo() {
        let (mods, vk) = parse_hotkey_combo("ctrl+shift+i").unwrap();
        assert_eq!(mods & 0x0002, 0x0002); // MOD_CONTROL
        assert_eq!(mods & 0x0004, 0x0004); // MOD_SHIFT
        assert_eq!(vk, 0x49); // 'I'

        let (space_mods, space_vk) = parse_hotkey_combo("ctrl+space").unwrap();
        assert_eq!(space_mods & 0x0002, 0x0002); // MOD_CONTROL
        assert_eq!(space_vk, 0x20); // Space

        let (mods2, vk2) = parse_hotkey_combo("win+shift+f1").unwrap();
        assert_eq!(mods2 & 0x0008, 0x0008); // MOD_WIN
        assert_eq!(mods2 & 0x0004, 0x0004); // MOD_SHIFT
        assert_eq!(vk2, 0x70); // F1

        assert!(parse_hotkey_combo("invalid+modifier+k").is_err());
    }
}
