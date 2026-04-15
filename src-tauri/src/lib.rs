mod audio;
mod google_auth;
mod logging;
mod state;
mod transcription;

use serde::Serialize;
use state::AppState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State,
};

// ─── Tauri Commands ───

#[tauri::command]
async fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    {
        let lock = state.recording.lock().map_err(|e| e.to_string())?;
        if lock.is_some() {
            return Err("Optagelse er allerede i gang".into());
        }
    }

    let app_clone = app.clone();
    let models_dir = state.models_dir();

    // All audio init must run on a real OS thread (not tokio async) because
    // cpal and ScreenCaptureKit need an active run loop / thread context.
    let result = tokio::task::spawn_blocking(move || -> Result<state::ActiveRecording, String> {
        let mic = audio::capture::MicCapture::new().map_err(|e| format!("Mikrofon-fejl: {}", e))?;

        // System audio capture is optional — may fail if Screen Recording permission is not granted
        let system = match audio::platform::create_system_capture() {
            Ok(s) => s,
            Err(e) => {
                app_log!("[call-recorder] Systemlyd-optagelse fejlede (kun mikrofon): {}", e);
                let _ = app_clone.emit("recording-warning", format!(
                    "Systemlyd kunne ikke optages: {}. Kun mikrofon optages. Giv 'Screen Recording'-tilladelse i Systemindstillinger for at optage begge sider.",
                    e
                ));
                audio::platform::create_dummy_capture()
            }
        };

        mic.start().map_err(|e| format!("Kunne ikke starte mikrofon: {}", e))?;

        // start_capture() can hang forever if ScreenCaptureKit connection fails.
        // Run it on a separate thread with a timeout.
        let system = {
            let (tx, rx) = std::sync::mpsc::channel();
            let sys_inner = system;
            std::thread::spawn(move || {
                let result = sys_inner.start();
                let _ = tx.send((result, sys_inner));
            });
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok((Ok(()), sys_back)) => sys_back,
                Ok((Err(e), _sys_back)) => {
                    app_log!("[call-recorder] Systemlyd start fejl: {}", e);
                    let _ = app_clone.emit("recording-warning",
                        "Systemlyd kunne ikke starte. Kun mikrofon optages.".to_string());
                    audio::platform::create_dummy_capture()
                }
                Err(_) => {
                    app_log!("[call-recorder] Systemlyd start timeout (5s) — bruger kun mikrofon");
                    let _ = app_clone.emit("recording-warning",
                        "Systemlyd-optagelse timed out. Kun mikrofon optages. Tjek Screen Recording-tilladelse.".to_string());
                    audio::platform::create_dummy_capture()
                }
            }
        };

        let id = uuid::Uuid::new_v4().to_string();
        app_log!("[call-recorder] Optagelse startet: {}", id);

        // Start live transcription if whisper model is available
        let live_transcriber = if transcription::model::is_model_downloaded(&models_dir) {
            let mic_arc = mic.samples_arc();
            let sys_arc = system.samples_arc();
            let mic_rate = mic.sample_rate();
            let sys_rate = system.sample_rate();
            let model = transcription::model::model_path(&models_dir);
            Some(transcription::live::LiveTranscriber::start(
                id.clone(),
                mic_arc,
                mic_rate,
                sys_arc,
                sys_rate,
                model,
                app_clone.clone(),
            ))
        } else {
            None
        };

        Ok(state::ActiveRecording {
            id,
            mic,
            system,
            live_transcriber,
        })
    })
    .await
    .map_err(|e| format!("Thread-fejl: {}", e))?;

    let active = result?;
    let id = active.id.clone();

    let mut lock = state.recording.lock().map_err(|e| e.to_string())?;
    *lock = Some(active);

    Ok(id)
}

#[derive(Serialize)]
struct StopRecordingResult {
    id: String,
    duration_secs: f64,
    has_system_audio: bool,
}

#[tauri::command]
async fn stop_recording(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StopRecordingResult, String> {
    let mut active = {
        let mut lock = state.recording.lock().map_err(|e| e.to_string())?;
        lock.take().ok_or("Ingen aktiv optagelse")?
    };

    // Stop live transcriber before stopping audio capture
    if let Some(lt) = active.live_transcriber.take() {
        lt.stop();
    }

    let recording_id = active.id.clone();
    let recordings_dir = state.recordings_dir();
    std::fs::create_dir_all(&recordings_dir).map_err(|e| e.to_string())?;

    let rec_dir = recordings_dir.clone();
    let rid = recording_id.clone();

    // Stop capture + save WAVs on a blocking thread (ScreenCaptureKit needs it)
    let saved = tokio::task::spawn_blocking(move || -> Result<audio::mixer::SavedAudio, String> {
        let mut active = active;
        if let Err(e) = active.mic.stop() {
            app_log!("[call-recorder] Mic stop fejl: {}", e);
        }
        if let Err(e) = active.system.stop() {
            app_log!("[call-recorder] System stop fejl: {}", e);
        }

        let mic_samples = active.mic.take_samples();
        let mic_rate = active.mic.sample_rate();
        let sys_samples = active.system.take_samples();
        let sys_rate = active.system.sample_rate();

        app_log!("[call-recorder] Optagelse stoppet. Mic: {}, System: {}", mic_samples.len(), sys_samples.len());

        audio::mixer::save_separate_and_mixed(
            &mic_samples, mic_rate,
            &sys_samples, sys_rate,
            &rec_dir, &rid,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Thread-fejl: {}", e))??;

    // Auto-transcribe in background
    let has_system = saved.has_system_audio;
    let models_dir = state.models_dir();
    let rec_dir = recordings_dir.clone();
    let transcribe_id = recording_id.clone();
    let app_clone = app.clone();

    tokio::spawn(async move {
        auto_transcribe(transcribe_id, has_system, rec_dir, models_dir, app_clone).await;
    });

    Ok(StopRecordingResult {
        id: recording_id,
        duration_secs: saved.duration_secs,
        has_system_audio: saved.has_system_audio,
    })
}

async fn auto_transcribe(
    id: String,
    has_system: bool,
    recordings_dir: std::path::PathBuf,
    models_dir: std::path::PathBuf,
    app: AppHandle,
) {
    if !transcription::model::is_model_downloaded(&models_dir) {
        return;
    }

    let _ = app.emit("transcription-started", &id);

    let mic_path = recordings_dir.join(format!("{}_mic.wav", id));
    let sys_path = recordings_dir.join(format!("{}_system.wav", id));
    let model = transcription::model::model_path(&models_dir);

    let result = tokio::task::spawn_blocking(move || -> Result<String, anyhow::Error> {
        use transcription::whisper::{Transcriber, merge_segments, format_transcript};

        let transcriber = Transcriber::new(&model)?;

        let mic_segments = if mic_path.exists() {
            match transcriber.transcribe_channel(&mic_path, "Sælger") {
                Ok(segs) => segs,
                Err(e) => {
                    app_log!("[call-recorder] Sælger transskription fejl: {}", e);
                    Vec::new()
                }
            }
        } else {
            app_log!("[call-recorder] Ingen mic-fil fundet: {:?}", mic_path);
            Vec::new()
        };

        let sys_segments = if has_system && sys_path.exists() {
            match transcriber.transcribe_channel(&sys_path, "Lead") {
                Ok(segs) => segs,
                Err(e) => {
                    app_log!("[call-recorder] Lead transskription fejl: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let merged = merge_segments(mic_segments, sys_segments);
        Ok(format_transcript(&merged))
    })
    .await;

    match result {
        Ok(Ok(text)) if !text.is_empty() => {
            let _ = app.emit("transcription-complete", serde_json::json!({"id": id, "text": text}));
        }
        _ => {
            let _ = app.emit("transcription-failed", &id);
        }
    }
}

#[tauri::command]
async fn delete_wav_files(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.recordings_dir();
    for suffix in &[".wav", "_mic.wav", "_system.wav"] {
        let path = dir.join(format!("{}{}", id, suffix));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn get_recording_wav_path(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let path = state.recordings_dir().join(format!("{}.wav", id));
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
async fn transcribe_recording(
    id: String,
    has_system_audio: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let recordings_dir = state.recordings_dir();
    let models_dir = state.models_dir();

    if !transcription::model::is_model_downloaded(&models_dir) {
        return Err("Whisper-model er ikke downloadet endnu".into());
    }

    tokio::spawn(async move {
        auto_transcribe(id, has_system_audio, recordings_dir, models_dir, app).await;
    });

    Ok(())
}

#[tauri::command]
async fn google_sign_in() -> Result<google_auth::GoogleTokens, String> {
    google_auth::authenticate()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_model_status(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(transcription::model::is_model_downloaded(&state.models_dir()))
}

#[tauri::command]
async fn download_model(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let models_dir = state.models_dir();
    transcription::model::download_model(&models_dir, &app)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn is_recording(state: State<'_, AppState>) -> Result<bool, String> {
    let lock = state.recording.lock().map_err(|e| e.to_string())?;
    Ok(lock.is_some())
}

#[tauri::command]
async fn get_logs() -> Result<Vec<String>, String> {
    Ok(logging::get_logs())
}

// ─── App Setup ───

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let start = MenuItem::with_id(app, "start", "Start optagelse", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop optagelse", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Åbn optagelser", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Afslut", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&start, &stop, &open, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "start" => {
                let _ = app.emit("tray-start-recording", ());
            }
            "stop" => {
                let _ = app.emit("tray-stop-recording", ());
            }
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("Kunne ikke finde app data mappe");
            std::fs::create_dir_all(&data_dir)?;

            let state = AppState::new(data_dir);
            app.manage(state);

            // Hide dock icon — must happen before any window is shown
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            setup_tray(app.handle())?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            delete_wav_files,
            get_recording_wav_path,
            transcribe_recording,
            google_sign_in,
            check_model_status,
            download_model,
            is_recording,
            get_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
