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

pub struct AppState {
    pub recording: Arc<Mutex<Option<ActiveRecording>>>,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            recording: Arc::new(Mutex::new(None)),
            data_dir,
        }
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
}
