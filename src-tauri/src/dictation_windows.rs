// Windows: hold Ctrl+Space -> record mic -> transcribe -> insert text.
#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};

use crate::app_log;
use crate::settings::{Settings, TranscriptionProvider};
use crate::transcription::{model, whisper::Transcriber};
use crate::{
    audio::capture::MicCapture,
    audio::mixer::resample,
    text_insert,
};
use tauri::{Emitter, Manager};

type Hhook = isize;
type Hwnd = isize;
type Hinstance = isize;
type Lparam = isize;
type Lresult = isize;
type Wparam = usize;

const WH_KEYBOARD_LL: i32 = 13;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const VK_CONTROL: i32 = 0x11;
const VK_SPACE: u32 = 0x20;

#[repr(C)]
#[derive(Clone, Copy)]
struct Kbdllhookstruct {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: Wparam,
    l_param: Lparam,
    time: u32,
    pt: Point,
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        id_hook: i32,
        lpfn: Option<unsafe extern "system" fn(i32, Wparam, Lparam) -> Lresult>,
        hmod: Hinstance,
        dw_thread_id: u32,
    ) -> Hhook;
    fn CallNextHookEx(hhk: Hhook, n_code: i32, w_param: Wparam, l_param: Lparam) -> Lresult;
    fn GetMessageW(
        lp_msg: *mut Msg,
        hwnd: Hwnd,
        w_msg_filter_min: u32,
        w_msg_filter_max: u32,
    ) -> i32;
    fn GetAsyncKeyState(v_key: i32) -> i16;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lp_module_name: *const u16) -> Hinstance;
}

static HOTKEY_TX: OnceLock<mpsc::SyncSender<bool>> = OnceLock::new();
static PREV_HOTKEY_DOWN: AtomicBool = AtomicBool::new(false);

fn extract_anna_command(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if lower.starts_with("anna") {
        let rest = text["anna".len()..].trim_start_matches([',', '.', ' ', '\n']);
        return Some(rest.to_string());
    }
    None
}

unsafe extern "system" fn keyboard_proc(
    n_code: i32,
    w_param: Wparam,
    l_param: Lparam,
) -> Lresult {
    if n_code >= 0 {
        let message = w_param as u32;
        let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;
        let kb = unsafe { *(l_param as *const Kbdllhookstruct) };
        let ctrl_down = unsafe { (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0 };
        let hotkey_active = PREV_HOTKEY_DOWN.load(Ordering::Relaxed);

        if kb.vk_code == VK_SPACE && ((is_down && ctrl_down) || (is_up && hotkey_active)) {
            let new_state = is_down;
            let old_state = PREV_HOTKEY_DOWN.swap(new_state, Ordering::Relaxed);
            if old_state != new_state {
                if let Some(tx) = HOTKEY_TX.get() {
                    let _ = tx.try_send(new_state);
                }
            }
            return 1;
        }
    }

    unsafe { CallNextHookEx(0 as Hhook, n_code, w_param, l_param) }
}

fn start_hotkey_thread(tx: mpsc::SyncSender<bool>) {
    HOTKEY_TX.set(tx).ok();

    std::thread::Builder::new()
        .name("dictation-windows-hotkey".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_proc),
                module,
                0,
            );

            if hook == 0 {
                app_log!("[dictation] Ctrl+Space monitor kunne ikke startes");
                return;
            }

            app_log!("[dictation] Ctrl+Space monitor aktiv");
            let mut msg: Msg = std::mem::zeroed();
            while GetMessageW(&mut msg, 0 as Hwnd, 0, 0) > 0 {}
        })
        .expect("Kunne ikke starte Windows hotkey-tråd");
}

fn show_microphone_issue(app: &tauri::AppHandle, error: &str) {
    app_log!("[dictation] Mikrofon ikke tilgængelig på Windows: {}", error);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("microphone-permission-missing", ());
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", "ms-settings:privacy-microphone"])
        .spawn();
}

pub fn start(app: tauri::AppHandle, data_dir: PathBuf) {
    let models_dir = data_dir.join("models");
    let (tx, rx) = mpsc::sync_channel::<bool>(8);
    start_hotkey_thread(tx);

    std::thread::Builder::new()
        .name("dictation-windows-coordinator".into())
        .spawn(move || {
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
            loop {
                let hotkey_down = match rx.recv() {
                    Ok(v) => v,
                    Err(_) => break,
                };

                if hotkey_down {
                    app_log!("[dictation] Ctrl+Space ned - starter mikrofon");
                    let _ = app.emit("dictation-recording", true);
                    crate::tray::set_icon(&app, crate::tray::TrayState::Recording);

                    match MicCapture::new() {
                        Ok(m) => match m.start() {
                            Ok(()) => mic = Some(m),
                            Err(e) => {
                                show_microphone_issue(&app, &e.to_string());
                                crate::tray::set_icon(&app, crate::tray::TrayState::Normal);
                                let _ = app.emit("dictation-recording", false);
                            }
                        },
                        Err(e) => {
                            show_microphone_issue(&app, &e.to_string());
                            crate::tray::set_icon(&app, crate::tray::TrayState::Normal);
                            let _ = app.emit("dictation-recording", false);
                        }
                    }
                } else {
                    app_log!("[dictation] Ctrl+Space op - stopper og transskriberer");
                    let _ = app.emit("dictation-recording", false);
                    crate::tray::set_icon(&app, crate::tray::TrayState::Thinking);

                    if let Some(mut m) = mic.take() {
                        let _ = m.stop();
                        let raw = m.take_samples();
                        let rate = m.sample_rate();
                        drop(m);

                        if raw.is_empty() {
                            crate::tray::set_icon(&app, crate::tray::TrayState::Normal);
                            continue;
                        }

                        let samples = resample(&raw, rate);
                        let t_arc = transcriber.clone();
                        let models = models_dir.clone();
                        let app_c = app.clone();
                        let data = data_dir.clone();
                        std::thread::spawn(move || {
                            let settings = Settings::load(&data);
                            let openai_key = app_c.state::<crate::state::AppState>().get_openai_key();
                            let transcription = match settings.transcription_provider {
                                TranscriptionProvider::Local => {
                                    let mut guard = t_arc.lock().unwrap();

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
                                    let Some(api_key) = openai_key
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
                                    let raw = segs
                                        .iter()
                                        .map(|s| s.text.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    let text = crate::transcription::cleaner::clean(&raw);
                                    app_log!("[dictation] Transskriberet: {} -> {}", raw, text);

                                    if let Some(command) = extract_anna_command(&text) {
                                        let screenshot_for_anna = if crate::anna::needs_screenshot(&command) {
                                            match crate::anna::take_screenshot_for_prompt(&command) {
                                                Ok(b) => Some(b),
                                                Err(e) => {
                                                    app_log!("[dictation] Screenshot ikke tilgængeligt: {}", e);
                                                    None
                                                }
                                            }
                                        } else {
                                            None
                                        };
                                        crate::anna::handle_query(&app_c, &command, screenshot_for_anna, &data);
                                    } else {
                                        crate::tray::set_icon(&app_c, crate::tray::TrayState::Normal);
                                        if let Err(e) = text_insert::insert_text(&text) {
                                            app_log!("[dictation] Indsættelsesfejl: {}", e);
                                            let _ = app_c.emit("dictation-error", e);
                                        }
                                    }
                                }
                                Ok(_) => {
                                    app_log!("[dictation] Ingen tale registreret");
                                    crate::tray::set_icon(&app_c, crate::tray::TrayState::Normal);
                                }
                                Err(e) => {
                                    app_log!("[dictation] Transskription fejl: {}", e);
                                    crate::tray::set_icon(&app_c, crate::tray::TrayState::Normal);
                                }
                            }
                        });
                    } else {
                        crate::tray::set_icon(&app, crate::tray::TrayState::Normal);
                    }
                }
            }
        })
        .expect("Kunne ikke starte Windows dictation-tråd");
}
