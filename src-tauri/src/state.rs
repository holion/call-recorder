use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::audio::capture::MicCapture;
use crate::audio::platform::SystemAudioCaptureHandle;
use crate::transcription::live::LiveTranscriber;

pub struct ActiveRecording {
    pub id: String,
    pub mic: MicCapture,
    pub system: SystemAudioCaptureHandle,
    pub live_transcriber: Option<LiveTranscriber>,
}

// Safety: MicCapture and SystemAudioCaptureHandle are Send
unsafe impl Send for ActiveRecording {}

pub struct AnnaStateData {
    pub prompt: String,
    pub response: Option<String>,
    pub insert_text: Option<String>,
    pub target_bundle_id: Option<String>,
    pub is_error: bool,
}

pub struct AppState {
    pub recording: Arc<Mutex<Option<ActiveRecording>>>,
    pub data_dir: PathBuf,
    pub anna: Mutex<Option<AnnaStateData>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            recording: Arc::new(Mutex::new(None)),
            data_dir,
            anna: Mutex::new(None),
        }
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
}
