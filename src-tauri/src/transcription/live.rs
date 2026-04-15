use crate::app_log;
use crate::audio::mixer;
use crate::transcription::whisper::{merge_segments, format_transcript, Transcriber};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const CHUNK_INTERVAL: Duration = Duration::from_secs(15);
/// Minimum 20 seconds of 16kHz audio before transcribing a chunk
const MIN_NEW_SAMPLES_16K: usize = 16000 * 20;
/// 3 seconds overlap to avoid cutting words at chunk boundaries
const OVERLAP_SAMPLES_16K: usize = 16000 * 3;

pub struct LiveTranscriber {
    stop_signal: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

// Safety: JoinHandle and Arc<AtomicBool> are Send
unsafe impl Send for LiveTranscriber {}

impl LiveTranscriber {
    pub fn start(
        recording_id: String,
        mic_samples: Arc<Mutex<Vec<f32>>>,
        mic_rate: u32,
        sys_samples: Arc<Mutex<Vec<f32>>>,
        sys_rate: u32,
        model_path: PathBuf,
        app: AppHandle,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let handle = std::thread::spawn(move || {
            app_log!("[live] Starter live transskription...");

            let transcriber = match Transcriber::new(&model_path) {
                Ok(t) => {
                    app_log!("[live] Whisper-model indlæst");
                    t
                }
                Err(e) => {
                    app_log!("[live] Kunne ikke indlæse Whisper-model: {}", e);
                    return;
                }
            };

            let mut mic_cursor: usize = 0;
            let mut sys_cursor: usize = 0;
            let mut last_mic_end_ms: i64 = 0;
            let mut last_sys_end_ms: i64 = 0;

            loop {
                // Sleep in small increments so we can respond to stop signal quickly
                for _ in 0..(CHUNK_INTERVAL.as_millis() / 500) {
                    if stop_clone.load(Ordering::Relaxed) {
                        app_log!("[live] Stop-signal modtaget");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }

                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Calculate overlap in source sample rate
                let mic_overlap_src = (OVERLAP_SAMPLES_16K as f64 * mic_rate as f64 / 16000.0) as usize;
                let sys_overlap_src = (OVERLAP_SAMPLES_16K as f64 * sys_rate as f64 / 16000.0) as usize;

                // Read mic samples from overlap start
                let (mic_chunk, mic_new_cursor, mic_offset_samples) = {
                    let buf = mic_samples.lock().unwrap();
                    let start = mic_cursor.saturating_sub(mic_overlap_src);
                    if buf.len() > start {
                        (buf[start..].to_vec(), buf.len(), start)
                    } else {
                        (Vec::new(), mic_cursor, mic_cursor)
                    }
                };

                // Read system samples from overlap start
                let (sys_chunk, sys_new_cursor, sys_offset_samples) = {
                    let buf = sys_samples.lock().unwrap();
                    let start = sys_cursor.saturating_sub(sys_overlap_src);
                    if buf.len() > start {
                        (buf[start..].to_vec(), buf.len(), start)
                    } else {
                        (Vec::new(), sys_cursor, sys_cursor)
                    }
                };

                // Resample chunks to 16kHz
                let mic_resampled = mixer::resample(&mic_chunk, mic_rate);
                let sys_resampled = mixer::resample(&sys_chunk, sys_rate);

                // Check if we have enough new audio
                let mic_new_16k = if mic_resampled.len() > OVERLAP_SAMPLES_16K {
                    mic_resampled.len() - OVERLAP_SAMPLES_16K
                } else {
                    mic_resampled.len()
                };

                if mic_new_16k < MIN_NEW_SAMPLES_16K {
                    continue;
                }

                app_log!("[live] Transskriberer chunk: mic={} sys={} samples (16kHz)",
                    mic_resampled.len(), sys_resampled.len());

                // Time offset: where this chunk starts in the overall recording
                let mic_offset_ms = (mic_offset_samples as f64 / mic_rate as f64 * 1000.0) as i64;
                let sys_offset_ms = (sys_offset_samples as f64 / sys_rate as f64 * 1000.0) as i64;

                // Transcribe mic chunk
                let mic_segments = if !mic_resampled.is_empty() {
                    match transcriber.transcribe_audio(&mic_resampled, "Sælger") {
                        Ok(mut segs) => {
                            for seg in &mut segs {
                                seg.start_ms += mic_offset_ms;
                                seg.end_ms += mic_offset_ms;
                            }
                            // Deduplicate: skip segments before last emitted end
                            segs.retain(|s| s.start_ms >= last_mic_end_ms);
                            if let Some(last) = segs.last() {
                                last_mic_end_ms = last.end_ms;
                            }
                            segs
                        }
                        Err(e) => {
                            app_log!("[live] Sælger transskription fejl: {}", e);
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };

                // Transcribe system chunk
                let sys_segments = if !sys_resampled.is_empty() {
                    match transcriber.transcribe_audio(&sys_resampled, "Lead") {
                        Ok(mut segs) => {
                            for seg in &mut segs {
                                seg.start_ms += sys_offset_ms;
                                seg.end_ms += sys_offset_ms;
                            }
                            segs.retain(|s| s.start_ms >= last_sys_end_ms);
                            if let Some(last) = segs.last() {
                                last_sys_end_ms = last.end_ms;
                            }
                            segs
                        }
                        Err(e) => {
                            app_log!("[live] Lead transskription fejl: {}", e);
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };

                let all = merge_segments(mic_segments, sys_segments);
                if !all.is_empty() {
                    let text = format_transcript(&all);
                    let _ = app.emit("transcription-chunk", serde_json::json!({
                        "id": recording_id,
                        "text": text,
                    }));
                    app_log!("[live] Emitterede {} segmenter", all.len());
                }

                mic_cursor = mic_new_cursor;
                sys_cursor = sys_new_cursor;
            }

            app_log!("[live] Live transskription afsluttet");
        });

        Self {
            stop_signal: stop,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
