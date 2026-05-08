#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::io::Write;
    use std::process::{Command, Stdio};

    type CGEventRef = *mut c_void;
    type CGEventFlags = u64;

    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x0010_0000;
    const VK_COMMAND: u16 = 0x37;
    const VK_V: u16 = 0x09;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
        fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, key_down: bool) -> CGEventRef;
        fn CGEventPost(tap: u32, event: CGEventRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    pub fn insert_text(text: &str) -> Result<(), String> {
        set_clipboard(text)?;
        std::thread::sleep(std::time::Duration::from_millis(80));
        simulate_paste()
    }

    pub fn activate_app_by_bundle_id(bundle_id: &str) -> Result<(), String> {
        let script = format!("tell application id \"{}\" to activate", bundle_id);
        let status = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .status()
            .map_err(|e| format!("Kunne ikke aktivere app {}: {}", bundle_id, e))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("Aktivering af app {} fejlede", bundle_id))
        }
    }

    fn set_clipboard(text: &str) -> Result<(), String> {
        let mut child = Command::new("pbcopy")
            .env("LANG", "en_US.UTF-8")
            .env("LC_CTYPE", "en_US.UTF-8")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy fejlede: {}", e))?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("Skriv til pbcopy fejlede: {}", e))?;
        }
        child
            .wait()
            .map_err(|e| format!("pbcopy afventing fejlede: {}", e))?;
        Ok(())
    }

    fn simulate_paste() -> Result<(), String> {
        unsafe {
            if !AXIsProcessTrusted() {
                return Err(
                    "Accessibility-tilladelse mangler — giv tilladelse i Systemindstillinger > Sikkerhed > Tilgængelighed".into()
                );
            }

            let cmd_down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), VK_COMMAND, true);
            let v_down = CGEventCreateKeyboardEvent(std::ptr::null_mut(), VK_V, true);
            let v_up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), VK_V, false);
            let cmd_up = CGEventCreateKeyboardEvent(std::ptr::null_mut(), VK_COMMAND, false);

            CGEventSetFlags(v_down, KCG_EVENT_FLAG_MASK_COMMAND);
            CGEventSetFlags(v_up, KCG_EVENT_FLAG_MASK_COMMAND);

            CGEventPost(KCG_HID_EVENT_TAP, cmd_down);
            CGEventPost(KCG_HID_EVENT_TAP, v_down);
            CGEventPost(KCG_HID_EVENT_TAP, v_up);
            CGEventPost(KCG_HID_EVENT_TAP, cmd_up);

            CFRelease(cmd_down as *const c_void);
            CFRelease(v_down as *const c_void);
            CFRelease(v_up as *const c_void);
            CFRelease(cmd_up as *const c_void);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use macos::{activate_app_by_bundle_id, insert_text};

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;
    use std::time::Duration;

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_CONTROL: u16 = 0x11;
    const VK_V: u16 = 0x56;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const GMEM_ZEROINIT: u32 = 0x0040;

    #[repr(C)]
    struct Input {
        r#type: u32,
        u: InputUnion,
    }

    #[repr(C)]
    union InputUnion {
        ki: KeybdInput,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeybdInput {
        w_vk: u16,
        w_scan: u16,
        dw_flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    #[link(name = "user32")]
    extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
        fn OpenClipboard(hwnd_new_owner: isize) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(u_format: u32) -> *mut c_void;
        fn SetClipboardData(u_format: u32, h_mem: *mut c_void) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(u_flags: u32, dw_bytes: usize) -> *mut c_void;
        fn GlobalLock(h_mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h_mem: *mut c_void) -> i32;
        fn GlobalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    pub fn insert_text(text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }

        let previous = read_clipboard_text();
        write_clipboard_text(text)?;
        std::thread::sleep(Duration::from_millis(80));
        let paste_result = simulate_ctrl_v();
        std::thread::sleep(Duration::from_millis(500));

        if let Some(previous) = previous {
            let _ = write_clipboard_text(&previous);
        }

        paste_result
    }

    fn read_clipboard_text() -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || OpenClipboard(0) == 0 {
                return None;
            }

            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                CloseClipboard();
                return None;
            }

            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                CloseClipboard();
                return None;
            }

            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            GlobalUnlock(handle);
            CloseClipboard();
            Some(text)
        }
    }

    fn write_clipboard_text(text: &str) -> Result<(), String> {
        let mut utf16 = text.encode_utf16().collect::<Vec<_>>();
        utf16.push(0);
        let bytes = utf16.len() * std::mem::size_of::<u16>();

        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
            if handle.is_null() {
                return Err("Windows clipboard allocation fejlede".into());
            }

            let locked = GlobalLock(handle) as *mut u16;
            if locked.is_null() {
                GlobalFree(handle);
                return Err("Windows clipboard lock fejlede".into());
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), locked, utf16.len());
            GlobalUnlock(handle);

            if OpenClipboard(0) == 0 {
                GlobalFree(handle);
                return Err("Windows clipboard kunne ikke åbnes".into());
            }

            EmptyClipboard();
            if SetClipboardData(CF_UNICODETEXT, handle).is_null() {
                CloseClipboard();
                GlobalFree(handle);
                return Err("Windows clipboard kunne ikke sættes".into());
            }

            CloseClipboard();
            Ok(())
        }
    }

    fn simulate_ctrl_v() -> Result<(), String> {
        let inputs = [
            key_input(VK_CONTROL, false),
            key_input(VK_V, false),
            key_input(VK_V, true),
            key_input(VK_CONTROL, true),
        ];

        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<Input>() as i32,
            )
        };

        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err(format!("Windows Ctrl+V sendte kun {}/{} input-events", sent, inputs.len()))
        }
    }

    fn key_input(vk: u16, key_up: bool) -> Input {
        Input {
            r#type: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: vk,
                    w_scan: 0,
                    dw_flags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        }
    }

    pub fn activate_app_by_bundle_id(_bundle_id: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use windows::{activate_app_by_bundle_id, insert_text};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn insert_text(_text: &str) -> Result<(), String> {
    Err("Tekstindsættelse er kun understøttet på macOS og Windows".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn activate_app_by_bundle_id(_bundle_id: &str) -> Result<(), String> {
    Ok(())
}
