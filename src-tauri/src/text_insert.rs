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

#[cfg(not(target_os = "macos"))]
pub fn insert_text(_text: &str) -> Result<(), String> {
    Err("Tekstindsættelse er kun understøttet på macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn activate_app_by_bundle_id(_bundle_id: &str) -> Result<(), String> {
    Ok(())
}
