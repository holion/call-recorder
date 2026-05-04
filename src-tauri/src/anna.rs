// Anna AI assistant: screenshot + OpenAI API call

use std::ffi::{c_void, CString};
use std::path::Path;
use std::process::Command;

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

        let type_str = CString::new("public.jpeg").unwrap();
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

fn frontmost_app_bundle_id() -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(
            "tell application \"System Events\" to get bundle identifier of first process whose frontmost is true",
        )
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let bundle = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if bundle.is_empty() {
        return None;
    }

    // Ignore our own app id if it happens to be frontmost at capture time.
    if bundle == "dk.holion.call-recorder" {
        return None;
    }

    Some(bundle)
}

// ── OpenAI call ────────────────────────────────────────────────────────

const SYSTEM: &str = "Du er Anna, en hjælpsom AI-assistent for et dansk salgsteam. \
    Svar præcist og kortfattet på dansk. \
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
) -> Result<(AnnaPayload, bool), String> {
    let user_content = match screenshot {
        Some(bytes) => {
            let encoded = B64.encode(&bytes);
            serde_json::json!([
                { "type": "text", "text": guidance_text },
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": {
                    "url": format!("data:image/jpeg;base64,{}", encoded),
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
) -> Result<AnnaPayload, String> {
    let primary_guidance = "Der er vedhæftet et skærmbillede. Brug det aktivt til dit svar. \
        Skriv ikke, at du ikke kan se billedet. Hvis noget er uklart, skriv præcist hvad der er uklart. \
        Tilpas formalitet og længde til mediet i screenshotet (mail vs chat).";

    let (first_payload, first_refusal) =
        call_openai_once(api_key, prompt, primary_guidance, screenshot.clone())?;

    if !first_refusal {
        return Ok(first_payload);
    }

    app_log!(
        "[anna] Refusal detekteret. Forsøger én retry med snævrere prompt (screenshot beholdes)."
    );

    let retry_guidance = "Brugeren ønsker hjælp til at formulere et venligt, harmløst tekstsvar på dansk. \
        Svar kort, konkret og uden ekstra sikkerhedsformuleringer.";
    let (retry_payload, retry_refusal) =
        call_openai_once(api_key, prompt, retry_guidance, screenshot.clone())?;
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
            call_openai_once(api_key, prompt, strict_guidance, screenshot.clone())?;
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

pub fn handle_query(app: &AppHandle, command: &str, screenshot: Option<Vec<u8>>, data_dir: &Path) {
    let settings = Settings::load(data_dir);
    let target_bundle_id = frontmost_app_bundle_id();

    let api_key = match settings.openai_api_key.as_deref() {
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
    match call_openai(&api_key, command, needs_insert, used_screenshot.clone()) {
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
