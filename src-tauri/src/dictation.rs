// macOS-only: hold Fn → record mic → Whisper → insert via Accessibility API
#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};

use crate::app_log;
use crate::audio::capture::MicCapture;
use crate::audio::mixer::resample;
use crate::settings::{Settings, TranscriptionProvider};
use crate::transcription::{model, whisper::Transcriber};
use tauri::Emitter;

// ── FFI types ──────────────────────────────────────────────────────────

type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;
type CGEventType = u32;
type CGEventFlags = u64;

type EventTapCallback = extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

const KCG_HID_EVENT_TAP: u32 = 0;
const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const KCG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const KCG_EVENT_FLAGS_CHANGED: CGEventType = 12;
// kCGEventFlagMaskSecondaryFn — set when Fn is held
const KCG_EVENT_FLAG_MASK_SECONDARY_FN: CGEventFlags = 0x0080_0000;

// ── FFI declarations ───────────────────────────────────────────────────

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: EventTapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
    fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventCreateKeyboardEvent(source: *mut c_void, keycode: u16, key_down: bool) -> CGEventRef;
    fn CGEventPost(tap: u32, event: CGEventRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *mut c_void,
        port: CFMachPortRef,
        order: i64,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFRelease(cf: *const c_void);
    static kCFRunLoopCommonModes: CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

// ── Global event tap state ─────────────────────────────────────────────

static FN_TX: OnceLock<mpsc::SyncSender<bool>> = OnceLock::new();
static PREV_FN_DOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if event_type != KCG_EVENT_FLAGS_CHANGED {
        return event;
    }

    let flags = unsafe { CGEventGetFlags(event) };
    let fn_down = (flags & KCG_EVENT_FLAG_MASK_SECONDARY_FN) != 0;
    let was_down = PREV_FN_DOWN.swap(fn_down, Ordering::Relaxed);

    if fn_down == was_down {
        return event;
    }

    if let Some(tx) = FN_TX.get() {
        let _ = tx.try_send(fn_down);
    }

    // Suppress the Fn press so macOS doesn't open the emoji picker.
    if fn_down {
        return std::ptr::null_mut();
    }

    event
}

// ── Text insertion ─────────────────────────────────────────────────────
//
// kAXSelectedTextAttribute is read-only on macOS and silently ignores writes.
// The universal approach is: write text to clipboard via pbcopy, then post
// a Cmd+V keyboard event so the focused app pastes it.

const KCG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x0010_0000;
// kVK_Command = 0x37, kVK_ANSI_V = 0x09
const VK_COMMAND: u16 = 0x37;
const VK_V: u16 = 0x09;

fn insert_text(text: &str) -> Result<(), String> {
    set_clipboard(text)?;
    // Give the pasteboard time to propagate before sending Cmd+V.
    std::thread::sleep(std::time::Duration::from_millis(80));
    simulate_paste()
}

fn set_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

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

        // Mark v press/release as having Command held
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

// ── Entry point ────────────────────────────────────────────────────────

fn extract_anna_command(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for trigger in &["hey anna", "hej anna", "hey, anna", "hej, anna"] {
        if lower.starts_with(trigger) {
            let rest = text[trigger.len()..].trim_start_matches([',', '.', ' ', '\n']);
            return Some(rest.to_string());
        }
    }
    None
}

pub fn start(app: tauri::AppHandle, data_dir: PathBuf) {
    let models_dir = data_dir.join("models");
    let (tx, rx) = mpsc::sync_channel::<bool>(8);
    FN_TX.set(tx).ok();

    // Thread 1: CGEventTap running on its own CFRunLoop.
    std::thread::Builder::new()
        .name("dictation-event-tap".into())
        .spawn(|| unsafe {
            let mask = 1u64 << KCG_EVENT_FLAGS_CHANGED;
            let tap = CGEventTapCreate(
                KCG_HID_EVENT_TAP,
                KCG_HEAD_INSERT_EVENT_TAP,
                KCG_EVENT_TAP_OPTION_DEFAULT,
                mask,
                tap_callback,
                std::ptr::null_mut(),
            );

            if tap.is_null() {
                app_log!("[dictation] CGEventTapCreate fejlede — mangler Accessibility-tilladelse");
                return;
            }

            let source = CFMachPortCreateRunLoopSource(std::ptr::null_mut(), tap, 0);
            let rl = CFRunLoopGetCurrent();
            CFRunLoopAddSource(rl, source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);
            app_log!("[dictation] Fn-tast monitor aktiv");
            CFRunLoopRun();
        })
        .expect("Kunne ikke starte event tap-tråd");

    // Thread 2: Recording + transcription coordinator.
    // Whisper and MicCapture must run on real OS threads, not tokio tasks.
    std::thread::Builder::new()
        .name("dictation-coordinator".into())
        .spawn(move || {
            // Pre-load Whisper model so first dictation isn't slow.
            let transcriber: Arc<Mutex<Option<Transcriber>>> = Arc::new(Mutex::new(None));
            let settings = Settings::load(&data_dir);
            if settings.transcription_provider == TranscriptionProvider::Local
                && model::is_model_downloaded(&models_dir)
            {
                match Transcriber::new(&model::model_path(&models_dir)) {
                    Ok(t) => {
                        *transcriber.lock().unwrap() = Some(t);
                        app_log!("[dictation] Whisper-model indlæst");
                    }
                    Err(e) => app_log!("[dictation] Whisper indlæsning fejlede: {}", e),
                }
            }

            let mut mic: Option<MicCapture> = None;
            let mut pending_screenshot: Option<Vec<u8>> = None;

            loop {
                let fn_down = match rx.recv() {
                    Ok(v) => v,
                    Err(_) => break,
                };

                if fn_down {
                    app_log!("[dictation] Fn ned — starter mikrofon");
                    let _ = app.emit("dictation-recording", true);
                    // Capture screenshot immediately so it reflects what's on screen
                    // at the moment the user starts speaking (before they describe it).
                    pending_screenshot = match crate::anna::take_screenshot() {
                        Ok(b) => { app_log!("[dictation] Screenshot taget ({} bytes)", b.len()); Some(b) }
                        Err(e) => { app_log!("[dictation] Screenshot fejl: {}", e); None }
                    };
                    match MicCapture::new() {
                        Ok(m) => match m.start() {
                            Ok(()) => mic = Some(m),
                            Err(e) => app_log!("[dictation] Mikrofon start fejl: {}", e),
                        },
                        Err(e) => app_log!("[dictation] MicCapture fejl: {}", e),
                    }
                } else {
                    app_log!("[dictation] Fn op — stopper og transskriberer");
                    let _ = app.emit("dictation-recording", false);

                    if let Some(mut m) = mic.take() {
                        let _ = m.stop();
                        let raw = m.take_samples();
                        let rate = m.sample_rate();
                        drop(m);

                        if raw.is_empty() {
                            continue;
                        }

                        let samples = resample(&raw, rate);
                        let t_arc = transcriber.clone();
                        let models = models_dir.clone();
                        let app_c = app.clone();
                        let data = data_dir.clone();
                        let screenshot_for_anna = pending_screenshot.take();

                        std::thread::spawn(move || {
                            let settings = Settings::load(&data);
                            let transcription = match settings.transcription_provider {
                                TranscriptionProvider::Local => {
                                    let mut guard = t_arc.lock().unwrap();

                                    // Lazy-load model if not yet available
                                    if guard.is_none() && model::is_model_downloaded(&models) {
                                        *guard = Transcriber::new(&model::model_path(&models)).ok();
                                    }

                                    let Some(ref t) = *guard else {
                                        app_log!("[dictation] Whisper-model ikke tilgængelig");
                                        return;
                                    };

                                    t.transcribe_audio(&samples, "Diktation")
                                }
                                TranscriptionProvider::Openai => {
                                    let Some(api_key) = settings
                                        .openai_api_key
                                        .as_deref()
                                        .map(str::trim)
                                        .filter(|key| !key.is_empty())
                                        .map(str::to_string)
                                    else {
                                        app_log!("[dictation] OpenAI-transskription mangler API-nøgle");
                                        let _ = app_c.emit(
                                            "dictation-error",
                                            "OpenAI-transskription kræver en API-nøgle i indstillinger.".to_string(),
                                        );
                                        return;
                                    };

                                    crate::transcription::openai::transcribe_audio(
                                        &api_key,
                                        &samples,
                                        "Diktation",
                                    )
                                }
                            };

                            match transcription {
                                Ok(segs) if !segs.is_empty() => {
                                    let text = segs
                                        .iter()
                                        .map(|s| s.text.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    app_log!("[dictation] Transskriberet: {}", text);

                                    if let Some(command) = extract_anna_command(&text) {
                                        crate::anna::handle_query(&app_c, &command, screenshot_for_anna, &data);
                                    } else if let Err(e) = insert_text(&text) {
                                        app_log!("[dictation] Indsættelsesfejl: {}", e);
                                        let _ = app_c.emit("dictation-error", e);
                                    }
                                }
                                Ok(_) => {
                                    app_log!("[dictation] Ingen tale registreret");
                                }
                                Err(e) => {
                                    app_log!("[dictation] Transskription fejl: {}", e);
                                }
                            }
                        });
                    }
                }
            }
        })
        .expect("Kunne ikke starte dictation-tråd");
}
