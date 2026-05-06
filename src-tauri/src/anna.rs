// Anna AI assistant: screenshot + OpenAI API call

use std::ffi::{c_void, CString};
use std::path::Path;
use std::process::Command;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::app_log;
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
    #[serde(default)]
    finish_reason: Option<String>,
    message: ChoiceMsg,
}

#[derive(Deserialize)]
struct ChoiceMsg {
    #[serde(default)]
    content: String,
    #[serde(default)]
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct AnnaPayload {
    answer: String,
    #[serde(default)]
    insert_text: Option<String>,
}

// ── Screenshot via in-process CoreGraphics ────────────────────────────
//
// We capture using CGDisplayCreateImage instead of spawning `screencapture`,
// because a child process does NOT inherit our app's Screen Recording
// permission and produces a black image.
// The screenshot is encoded as JPEG to keep payload size down; Chat Completions
// can drop image inputs that are too large.

type CGDirectDisplayID = u32;
type CGImageRef = *mut c_void;
type CFMutableDataRef = *mut c_void;
type CFStringRef = *const c_void;
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}
#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}
type CGWindowID = u32;
type CGWindowListOption = u32;
type CGWindowImageOption = u32;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGDisplayCreateImage(display_id: CGDirectDisplayID) -> CGImageRef;
    fn CGImageRelease(image: CGImageRef);
    fn CGWindowListCreateImage(
        screen_bounds: CGRect,
        list_option: CGWindowListOption,
        window_id: CGWindowID,
        image_option: CGWindowImageOption,
    ) -> CGImageRef;
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
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: CGWindowListOption = 1 << 0;
const K_CG_WINDOW_IMAGE_DEFAULT: CGWindowImageOption = 0;
const K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING: CGWindowImageOption = 1 << 0;

fn frontmost_window_id() -> Option<CGWindowID> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to tell (first process whose frontmost is true) to get value of attribute \"AXWindowNumber\" of front window")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    txt.parse::<u32>().ok()
}

fn encode_cg_image_as_jpeg(image: CGImageRef) -> Result<Vec<u8>, String> {
    unsafe {
        let data = CFDataCreateMutable(std::ptr::null(), 0);
        if data.is_null() {
            return Err("CFDataCreateMutable fejlede".into());
        }

        let type_str = CString::new("public.jpeg").map_err(|e| format!("CString fejl: {}", e))?;
        let jpeg_cfstr = CFStringCreateWithCString(
            std::ptr::null(),
            type_str.as_ptr(),
            KCF_STRING_ENCODING_UTF8,
        );

        let dest = CGImageDestinationCreateWithData(data, jpeg_cfstr, 1, std::ptr::null());
        CFRelease(jpeg_cfstr as *const c_void);

        if dest.is_null() {
            CFRelease(data as *const c_void);
            return Err("CGImageDestinationCreateWithData fejlede".into());
        }

        CGImageDestinationAddImage(dest, image, std::ptr::null());
        let ok = CGImageDestinationFinalize(dest);
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

pub(crate) fn take_screenshot() -> Result<Vec<u8>, String> {
    unsafe {
        let display = CGMainDisplayID();
        let image = CGDisplayCreateImage(display);
        if image.is_null() {
            return Err(
                "CGDisplayCreateImage fejlede — mangler Screen Recording-tilladelse?".into(),
            );
        }

        let bytes = encode_cg_image_as_jpeg(image);
        CGImageRelease(image);
        bytes
    }
}

fn wants_full_screen(prompt: &str) -> bool {
    let l = prompt.to_lowercase();
    [
        "hele skærmen",
        "hele skærm",
        "hele desktop",
        "entire screen",
        "full screen",
    ]
    .iter()
    .any(|k| l.contains(k))
}

pub(crate) fn take_screenshot_for_prompt(prompt: &str) -> Result<Vec<u8>, String> {
    if wants_full_screen(prompt) {
        app_log!("[anna] Screenshot-mode: hele skærmen");
        return take_screenshot();
    }

    unsafe {
        if let Some(window_id) = frontmost_window_id() {
            let image = CGWindowListCreateImage(
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 0.0,
                        height: 0.0,
                    },
                },
                K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY,
                window_id,
                K_CG_WINDOW_IMAGE_DEFAULT | K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING,
            );
            if !image.is_null() {
                app_log!("[anna] Screenshot-mode: aktivt vindue");
                let bytes = encode_cg_image_as_jpeg(image);
                CGImageRelease(image);
                return bytes;
            }
        }
    }

    app_log!("[anna] Kunne ikke fange aktivt vindue; falder tilbage til hele skærmen");
    take_screenshot()
}

pub(crate) fn needs_screenshot(prompt: &str) -> bool {
    let l = prompt.to_lowercase();
    ["skærm", "skærmen", "skærmbillede", "screenshot", "se på"]
        .iter()
        .any(|k| l.contains(k))
}

fn wants_insert_text(prompt: &str) -> bool {
    let l = prompt.to_lowercase();
    [
        "hjælp mig med et svar",
        "hjælp mig med svar",
        "skriv et svar",
        "formuler et svar",
        "svar til",
        "svar på",
        "skriv en besked",
        "formuler en besked",
        "skriv en mail",
        "formuler en mail",
        "udkast",
    ]
    .iter()
    .any(|k| l.contains(k))
}

fn frontmost_app_info() -> (Option<String>, Option<String>) {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(
            "tell application \"System Events\"\n\
             set p to first process whose frontmost is true\n\
             return (bundle identifier of p) & \"\t\" & (name of p)\n\
             end tell",
        )
        .output()
        .ok();

    let output = match output {
        Some(o) if o.status.success() => o,
        _ => return (None, None),
    };

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = raw.splitn(2, '\t');
    let bundle = parts.next().unwrap_or("").to_string();
    let name = parts.next().unwrap_or("").to_string();

    // Ignore our own app if it happens to be frontmost at capture time.
    if bundle.is_empty() || bundle == "dk.holion.call-recorder" {
        return (None, None);
    }

    let name = if name.is_empty() { None } else { Some(name) };
    (Some(bundle), name)
}

fn is_chromium_browser(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "company.thebrowser.Browser" // Arc
            | "com.google.Chrome"
            | "com.brave.Browser"
            | "com.microsoft.edgemac"
            | "org.chromium.Chromium"
            | "com.operasoftware.Opera"
            | "com.vivaldi.Vivaldi"
    )
}

fn frontmost_browser_tab_title(app_name: &str, bundle_id: &str) -> Option<String> {
    let script = if bundle_id == "com.apple.Safari" {
        "tell application \"Safari\" to get name of current tab of front window".to_string()
    } else if is_chromium_browser(bundle_id) {
        format!(
            "tell application \"{}\" to get title of active tab of front window",
            app_name
        )
    } else {
        return None;
    };

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

// ── OpenAI call ────────────────────────────────────────────────────────

const SYSTEM: &str = "Du er Anna, en hjælpsom AI-assistent for et dansk salgsteam. \
    Svar præcist og kortfattet på samme sprog som i samtalen/teksten i screenshotet. \
    Hvis sproget i screenshotet er uklart, svar på dansk. \
    Når du foreslår tekst, skal du matche konteksten i screenshotet: \
    Hvis det ligner e-mail, så brug passende formel/professionel tone. \
    Hvis det ligner chat/Slack/Teams/SMS, så brug en kortere og mere uformel tone. \
    For chat/Slack/Teams/SMS: skriv 1-3 korte linjer, ingen emnelinje, \
    ingen e-mail-opbygning, og ingen signatur medmindre tråden tydeligt bruger signaturer. \
    Start ikke automatisk med 'Hej [navn],' i chat-kontekst medmindre den eksisterende tråd gør det. \
    Hvis mediet er uklart, vælg chat-stil frem for formel e-mail-stil. \
    Spejl så vidt muligt tonen i den tekst, der allerede står i tråden, medmindre brugeren beder om noget andet. \
    Returnér ALTID gyldig JSON med formatet: \
    {\"answer\":\"...\",\"insert_text\":\"... eller null\"}. \
    Brug kun insert_text når brugerens intention tydeligt er at få skrevet/formuleret tekst, \
    som skal indsættes et andet sted (fx svar, besked, mail, opslag, tekstudkast). \
    Brug null når intentionen ikke er tydelig. \
    Hvis intentionen ER tydelig, skal insert_text være den rene tekst der skal indsættes \
    (uden introduktioner som 'Du kan svare ved at skrive:').";

fn parse_anna_payload(raw: &str) -> AnnaPayload {
    if let Ok(payload) = serde_json::from_str::<AnnaPayload>(raw) {
        return payload;
    }

    // Fallback: tolerate fenced JSON outputs.
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                if let Ok(payload) = serde_json::from_str::<AnnaPayload>(&raw[start..=end]) {
                    return payload;
                }
            }
        }
    }

    AnnaPayload {
        answer: raw.trim().to_string(),
        insert_text: None,
    }
}

fn looks_like_refusal(text: &str, finish_reason: Option<&str>, refusal: Option<&str>) -> bool {
    if finish_reason == Some("content_filter") {
        return true;
    }
    if refusal.map(str::trim).filter(|s| !s.is_empty()).is_some() {
        return true;
    }

    let l = text.to_lowercase();
    [
        "i'm sorry, i can't assist with that",
        "i can't assist with that",
        "i can’t assist with that",
        "i'm sorry, i can't help with that",
        "jeg kan ikke hjælpe med det",
        "beklager, men jeg kan ikke hjælpe med det",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn call_openai_once(
    api_key: &str,
    prompt: &str,
    guidance_text: &str,
    screenshot: Option<Vec<u8>>,
    app_name: Option<&str>,
) -> Result<(AnnaPayload, bool), String> {
    let user_content = match screenshot {
        Some(bytes) => {
            let encoded = B64.encode(&bytes);
            let mut parts = vec![];
            if let Some(ctx) = app_name {
                parts.push(serde_json::json!({ "type": "text", "text": ctx }));
            }
            parts.push(serde_json::json!({ "type": "text", "text": guidance_text }));
            parts.push(serde_json::json!({ "type": "text", "text": prompt }));
            parts.push(serde_json::json!({ "type": "image_url", "image_url": {
                "url": format!("data:image/jpeg;base64,{}", encoded),
                "detail": "high"
            }}));
            serde_json::Value::Array(parts)
        }
        None => match app_name {
            Some(ctx) => serde_json::json!([
                { "type": "text", "text": ctx },
                { "type": "text", "text": prompt }
            ]),
            None => serde_json::json!(prompt),
        },
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

    let raw_json = resp
        .text()
        .map_err(|e| format!("Kunne ikke læse OpenAI JSON-svar: {}", e))?;
    app_log!("[anna] Rå OpenAI JSON: {}", raw_json);

    let parsed: Response = serde_json::from_str(&raw_json)
        .map_err(|e| format!("Kunne ikke parse OpenAI JSON-svar: {}", e))?;

    let first = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "Tomt svar fra OpenAI".to_string())?;

    let raw = first.message.content.trim().to_string();
    if raw.is_empty() {
        let refusal_text = first.message.refusal.unwrap_or_default();
        return Ok((
            AnnaPayload {
                answer: refusal_text.clone(),
                insert_text: None,
            },
            looks_like_refusal(
                &refusal_text,
                first.finish_reason.as_deref(),
                Some(&refusal_text),
            ),
        ));
    }

    app_log!("[anna] Råt OpenAI-svar: {}", raw);

    Ok((
        parse_anna_payload(&raw),
        looks_like_refusal(
            &raw,
            first.finish_reason.as_deref(),
            first.message.refusal.as_deref(),
        ),
    ))
}

fn call_openai(
    api_key: &str,
    prompt: &str,
    require_insert_text: bool,
    screenshot: Option<Vec<u8>>,
    app_name: Option<&str>,
) -> Result<AnnaPayload, String> {
    let primary_guidance = "Der er vedhæftet et skærmbillede. Brug det aktivt til dit svar. \
        Skriv ikke, at du ikke kan se billedet. Hvis noget er uklart, skriv præcist hvad der er uklart. \
        Tilpas formalitet og længde til mediet i screenshotet (mail vs chat).";

    let (first_payload, first_refusal) =
        call_openai_once(api_key, prompt, primary_guidance, screenshot.clone(), app_name)?;

    if !first_refusal {
        return Ok(first_payload);
    }

    app_log!(
        "[anna] Refusal detekteret. Forsøger én retry med snævrere prompt (screenshot beholdes)."
    );

    let retry_guidance = "Brugeren ønsker hjælp til at formulere et venligt, harmløst tekstsvar. \
        Brug samme sprog som i samtalen/teksten i screenshotet. Hvis uklart, brug dansk. \
        Svar kort, konkret og uden ekstra sikkerhedsformuleringer.";
    let (retry_payload, retry_refusal) =
        call_openai_once(api_key, prompt, retry_guidance, screenshot.clone(), app_name)?;
    if !retry_refusal {
        app_log!("[anna] Retry lykkedes uden refusal.");
        if !require_insert_text
            || retry_payload
                .insert_text
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_some()
        {
            return Ok(retry_payload);
        }
    }

    // If user intent clearly asks for generated text, force a strict format retry.
    if require_insert_text {
        app_log!("[anna] insert_text mangler. Forsøger format-retry med krav om insert_text.");
        let strict_guidance = "VIGTIGT: Returnér KUN gyldig JSON i formatet \
            {\"answer\":\"...\",\"insert_text\":\"...\"}. \
            insert_text MÅ IKKE være null eller tom. \
            insert_text skal være den nøjagtige tekst, der skal indsættes direkte, \
            uden introduktioner eller forklaringer.";
        let (strict_payload, _strict_refusal) =
            call_openai_once(api_key, prompt, strict_guidance, screenshot.clone(), app_name)?;
        if strict_payload
            .insert_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some()
        {
            app_log!("[anna] Format-retry lykkedes med insert_text.");
            return Ok(strict_payload);
        }
        app_log!("[anna] Format-retry gav stadig ikke insert_text.");
        return Ok(strict_payload);
    }

    Ok(retry_payload)
}

// ── Anna window ────────────────────────────────────────────────────────

fn set_anna_state(
    app: &AppHandle,
    prompt: &str,
    response: Option<&str>,
    insert_text: Option<&str>,
    target_bundle_id: Option<&str>,
    is_error: bool,
) {
    if let Ok(mut guard) = app.state::<AppState>().anna.lock() {
        *guard = Some(AnnaStateData {
            prompt: prompt.to_string(),
            response: response.map(|s| s.to_string()),
            insert_text: insert_text.map(|s| s.to_string()),
            target_bundle_id: target_bundle_id.map(|s| s.to_string()),
            is_error,
        });
    }
}

// ── Entry point called from dictation.rs ──────────────────────────────

pub fn handle_query(app: &AppHandle, command: &str, screenshot: Option<Vec<u8>>, _data_dir: &Path) {
    let openai_key = app.state::<crate::state::AppState>().get_openai_key();
    let (target_bundle_id, app_name) = frontmost_app_info();
    let tab_title = match (app_name.as_deref(), target_bundle_id.as_deref()) {
        (Some(name), Some(bundle)) => frontmost_browser_tab_title(name, bundle),
        _ => None,
    };
    app_log!("[anna] Aktiv app: {:?}, bundle: {:?}, tab: {:?}", app_name, target_bundle_id, tab_title);

    let app_context: Option<String> = match (app_name.as_deref(), tab_title.as_deref()) {
        (Some(name), Some(tab)) => Some(format!("Aktiv app: {}.\nAktiv tab: \"{}\".", name, tab)),
        (Some(name), None) => Some(format!("Aktiv app: {}.", name)),
        _ => None,
    };

    let api_key = match openai_key.as_deref() {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            app_log!("[anna] Ingen OpenAI API-nøgle konfigureret");
            set_anna_state(
                app,
                command,
                Some("Ingen OpenAI API-nøgle konfigureret. Åbn indstillinger (⚙)."),
                None,
                target_bundle_id.as_deref(),
                true,
            );
            let _ = app.emit("anna-show", ());
            return;
        }
    };

    // Show window immediately in thinking state — JS handles positioning + show
    set_anna_state(
        app,
        command,
        None,
        None,
        target_bundle_id.as_deref(),
        false,
    );
    let _ = app.emit("anna-show", ());
    let _ = app.emit("anna-thinking", serde_json::json!({ "prompt": command }));

    let used_screenshot = if needs_screenshot(command) {
        screenshot
    } else {
        None
    };
    if let Some(ref screenshot) = used_screenshot {
        app_log!(
            "[anna] Sender til OpenAI (inkl. screenshot, {} bytes): \"{}\"",
            screenshot.len(),
            command
        );
        if screenshot.len() > 8_000_000 {
            app_log!(
                "[anna] ADVARSEL: screenshot er over 8 MB; OpenAI kan ignorere billed-input i Chat Completions"
            );
        }
    } else {
        app_log!("[anna] Sender til OpenAI: \"{}\"", command);
    }

    crate::tray::set_icon(app, crate::tray::TrayState::Thinking);

    let needs_insert = wants_insert_text(command);
    match call_openai(&api_key, command, needs_insert, used_screenshot.clone(), app_context.as_deref()) {
        Ok(mut payload) => {
            if needs_insert
                && payload
                    .insert_text
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_none()
            {
                app_log!("[anna] insert_text mangler efter retries");
            }

            // Keep explanatory text in panel while preserving clean insert text.
            if let Some(ref draft) = payload.insert_text {
                payload.answer = draft.trim().to_string();
            }

            app_log!("[anna] Svar modtaget: {}", payload.answer);
            set_anna_state(
                app,
                command,
                Some(&payload.answer),
                payload.insert_text.as_deref(),
                target_bundle_id.as_deref(),
                false,
            );
            crate::tray::set_icon(app, crate::tray::TrayState::Normal);
            let _ = app.emit(
                "anna-response",
                serde_json::json!({
                    "prompt": command,
                    "response": payload.answer,
                    "insert_text": payload.insert_text,
                    "error": false
                }),
            );
        }
        Err(e) => {
            app_log!("[anna] Fejl: {}", e);
            let msg = format!("Fejl: {}", e);
            set_anna_state(
                app,
                command,
                Some(&msg),
                None,
                target_bundle_id.as_deref(),
                true,
            );
            crate::tray::set_icon(app, crate::tray::TrayState::Normal);
            let _ = app.emit(
                "anna-response",
                serde_json::json!({
                    "prompt": command,
                    "response": msg,
                    "insert_text": null,
                    "error": true
                }),
            );
        }
    }
}
