// Anna AI assistant: screenshot + OpenAI API call

use std::ffi::{c_void, CString};
use std::path::Path;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::app_log;
use crate::settings::Settings;
use crate::state::{AnnaStateData, AppState};

// ── OpenAI API types ───────────────────────────────────────────────────

#[derive(Serialize)]
struct Msg {
    role: &'static str,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct Request {
    model: &'static str,
    messages: Vec<Msg>,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMsg,
}

#[derive(Deserialize)]
struct ChoiceMsg {
    content: String,
}

// ── Screenshot via in-process CoreGraphics ────────────────────────────
//
// We capture using CGDisplayCreateImage instead of spawning `screencapture`,
// because a child process does NOT inherit our app's Screen Recording
// permission and produces a black image.

type CGDirectDisplayID = u32;
type CGImageRef = *mut c_void;
type CFMutableDataRef = *mut c_void;
type CFStringRef = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGDisplayCreateImage(display_id: CGDirectDisplayID) -> CGImageRef;
    fn CGImageRelease(image: CGImageRef);
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageDestinationCreateWithData(
        data: CFMutableDataRef,
        type_id: CFStringRef,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(dest: *mut c_void, image: CGImageRef, props: *const c_void);
    fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreateMutable(allocator: *const c_void, capacity: isize) -> CFMutableDataRef;
    fn CFDataGetLength(data: CFMutableDataRef) -> isize;
    fn CFDataGetBytePtr(data: CFMutableDataRef) -> *const u8;
    fn CFRelease(cf: *const c_void);
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const i8,
        encoding: u32,
    ) -> CFStringRef;
}

const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

pub(crate) fn take_screenshot() -> Result<Vec<u8>, String> {
    unsafe {
        let display = CGMainDisplayID();
        let image = CGDisplayCreateImage(display);
        if image.is_null() {
            return Err(
                "CGDisplayCreateImage fejlede — mangler Screen Recording-tilladelse?".into(),
            );
        }

        let data = CFDataCreateMutable(std::ptr::null(), 0);
        if data.is_null() {
            CGImageRelease(image);
            return Err("CFDataCreateMutable fejlede".into());
        }

        let type_str = CString::new("public.png").unwrap();
        let png_cfstr = CFStringCreateWithCString(
            std::ptr::null(),
            type_str.as_ptr() as *const i8,
            KCF_STRING_ENCODING_UTF8,
        );

        let dest = CGImageDestinationCreateWithData(data, png_cfstr, 1, std::ptr::null());
        CFRelease(png_cfstr as *const c_void);

        if dest.is_null() {
            CGImageRelease(image);
            CFRelease(data as *const c_void);
            return Err("CGImageDestinationCreateWithData fejlede".into());
        }

        CGImageDestinationAddImage(dest, image, std::ptr::null());
        let ok = CGImageDestinationFinalize(dest);
        CGImageRelease(image);
        CFRelease(dest as *const c_void);

        if !ok {
            CFRelease(data as *const c_void);
            return Err("CGImageDestinationFinalize fejlede".into());
        }

        let len = CFDataGetLength(data) as usize;
        let ptr = CFDataGetBytePtr(data);
        let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
        CFRelease(data as *const c_void);

        Ok(bytes)
    }
}

fn needs_screenshot(prompt: &str) -> bool {
    let l = prompt.to_lowercase();
    ["skærm", "skærmen", "skærmbillede", "screenshot", "se på"]
        .iter()
        .any(|k| l.contains(k))
}

// ── OpenAI call ────────────────────────────────────────────────────────

const SYSTEM: &str = "Du er Anna, en hjælpsom AI-assistent for et dansk salgsteam. \
    Svar præcist og kortfattet på dansk. Brug almindeligt sprog — ingen markdown-formatering.";

fn call_openai(api_key: &str, prompt: &str, screenshot: Option<Vec<u8>>) -> Result<String, String> {
    let user_content = match screenshot {
        Some(bytes) => {
            let encoded = B64.encode(&bytes);
            serde_json::json!([
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": {
                    "url": format!("data:image/png;base64,{}", encoded),
                    "detail": "high"
                }}
            ])
        }
        None => serde_json::json!(prompt),
    };

    let body = Request {
        model: "gpt-4o",
        messages: vec![
            Msg {
                role: "system",
                content: serde_json::json!(SYSTEM),
            },
            Msg {
                role: "user",
                content: user_content,
            },
        ],
        max_tokens: 1000,
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .map_err(|e| format!("Netværksfejl: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!("OpenAI API fejl {}: {}", status, text));
    }

    let parsed: Response = resp
        .json()
        .map_err(|e| format!("Kunne ikke parse svar: {}", e))?;

    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content.trim().to_string())
        .ok_or_else(|| "Tomt svar fra OpenAI".into())
}

// ── Anna window ────────────────────────────────────────────────────────

fn set_anna_state(app: &AppHandle, prompt: &str, response: Option<&str>, is_error: bool) {
    if let Ok(mut guard) = app.state::<AppState>().anna.lock() {
        *guard = Some(AnnaStateData {
            prompt: prompt.to_string(),
            response: response.map(|s| s.to_string()),
            is_error,
        });
    }
}

// ── Entry point called from dictation.rs ──────────────────────────────

pub fn handle_query(app: &AppHandle, command: &str, screenshot: Option<Vec<u8>>, data_dir: &Path) {
    let settings = Settings::load(data_dir);

    let api_key = match settings.openai_api_key.as_deref() {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            app_log!("[anna] Ingen OpenAI API-nøgle konfigureret");
            set_anna_state(
                app,
                command,
                Some("Ingen OpenAI API-nøgle konfigureret. Åbn indstillinger (⚙)."),
                true,
            );
            let _ = app.emit("anna-show", ());
            return;
        }
    };

    // Show window immediately in thinking state — JS handles positioning + show
    set_anna_state(app, command, None, false);
    let _ = app.emit("anna-show", ());
    let _ = app.emit("anna-thinking", serde_json::json!({ "prompt": command }));

    let used_screenshot = if needs_screenshot(command) {
        screenshot
    } else {
        None
    };
    if used_screenshot.is_some() {
        app_log!("[anna] Sender til OpenAI (inkl. screenshot): \"{}\"", command);
    } else {
        app_log!("[anna] Sender til OpenAI: \"{}\"", command);
    }

    crate::tray::set_icon(app, crate::tray::TrayState::Thinking);

    match call_openai(&api_key, command, used_screenshot) {
        Ok(response) => {
            app_log!("[anna] Svar modtaget: {}", response);
            set_anna_state(app, command, Some(&response), false);
            crate::tray::set_icon(app, crate::tray::TrayState::Normal);
            let _ = app.emit(
                "anna-response",
                serde_json::json!({
                    "prompt": command,
                    "response": response,
                    "error": false
                }),
            );
        }
        Err(e) => {
            app_log!("[anna] Fejl: {}", e);
            let msg = format!("Fejl: {}", e);
            set_anna_state(app, command, Some(&msg), true);
            crate::tray::set_icon(app, crate::tray::TrayState::Normal);
            let _ = app.emit(
                "anna-response",
                serde_json::json!({
                    "prompt": command,
                    "response": msg,
                    "error": true
                }),
            );
        }
    }
}
