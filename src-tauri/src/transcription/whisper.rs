use crate::app_log;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub speaker: String,
}

/// Shared transcriber that loads the model once
pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self> {
        app_log!("Indlæser Whisper-model...");
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().context("Ugyldig model-sti")?,
            WhisperContextParameters::default(),
        )
        .context("Kunne ikke indlæse Whisper-model")?;
        app_log!("Whisper-model indlæst");
        Ok(Self { ctx })
    }

    /// Transcribe a WAV file and return timed segments with speaker label
    pub fn transcribe_channel(
        &self,
        wav_path: &Path,
        speaker: &str,
    ) -> Result<Vec<TimedSegment>> {
        app_log!("Transskriberer {}...", speaker);

        let audio = load_wav_mono_f32(wav_path)?;
        if audio.is_empty() {
            app_log!("{}: tom lydfil", speaker);
            return Ok(Vec::new());
        }

        if !contains_speech(&audio, speaker) {
            return Ok(Vec::new());
        }

        let mut state = self.ctx.create_state().context("Kunne ikke oprette Whisper-state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("da"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);

        state
            .full(params, &audio)
            .context("Whisper-transskription fejlede")?;

        let num_segments = state.full_n_segments();
        let mut segments = Vec::new();

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str() {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        segments.push(TimedSegment {
                            start_ms: segment.start_timestamp(),
                            end_ms: segment.end_timestamp(),
                            text,
                            speaker: speaker.to_string(),
                        });
                    }
                }
            }
        }

        app_log!("{}: {} segmenter fundet", speaker, segments.len());
        Ok(segments)
    }

    /// Transcribe raw f32 audio samples (already at 16kHz mono) without file I/O
    pub fn transcribe_audio(
        &self,
        audio: &[f32],
        speaker: &str,
    ) -> Result<Vec<TimedSegment>> {
        if audio.is_empty() || !contains_speech(audio, speaker) {
            return Ok(Vec::new());
        }

        let mut state = self.ctx.create_state().context("Kunne ikke oprette Whisper-state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("da"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);

        state
            .full(params, audio)
            .context("Whisper-transskription fejlede")?;

        let num_segments = state.full_n_segments();
        let mut segments = Vec::new();

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str() {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        segments.push(TimedSegment {
                            start_ms: segment.start_timestamp(),
                            end_ms: segment.end_timestamp(),
                            text,
                            speaker: speaker.to_string(),
                        });
                    }
                }
            }
        }

        Ok(segments)
    }
}

/// Merge two sets of timed segments, sorted by start time
pub fn merge_segments(mut mic_segments: Vec<TimedSegment>, mut sys_segments: Vec<TimedSegment>) -> Vec<TimedSegment> {
    let mut all = Vec::new();
    all.append(&mut mic_segments);
    all.append(&mut sys_segments);
    all.sort_by_key(|s| s.start_ms);
    all
}

/// Format merged segments into readable text with speaker labels
pub fn format_transcript(segments: &[TimedSegment]) -> String {
    if segments.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    let mut current_speaker = String::new();

    for seg in segments {
        if seg.speaker != current_speaker {
            current_speaker = seg.speaker.clone();
            lines.push(format!("\n{}:\n{}", seg.speaker, seg.text));
        } else {
            lines.push(seg.text.clone());
        }
    }

    lines.join(" ").trim().to_string()
}

/// Check if audio contains actual speech by measuring RMS energy in 2-second windows.
/// If ANY window has sufficient energy, we consider the file to contain speech.
/// This avoids rejecting files where speech is surrounded by long periods of silence.
fn contains_speech(samples: &[f32], speaker: &str) -> bool {
    if samples.is_empty() {
        return false;
    }

    // 16 kHz * 2 seconds = 32000 samples per window
    let window_size = 32000;
    let mut max_rms: f64 = 0.0;
    let mut max_peak: f32 = 0.0;
    let mut speech_windows = 0usize;
    let total_windows = (samples.len() + window_size - 1) / window_size;

    for chunk in samples.chunks(window_size) {
        let sum_sq: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / chunk.len() as f64).sqrt();
        let peak = chunk.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        if rms > max_rms { max_rms = rms; }
        if peak > max_peak { max_peak = peak; }

        if rms > 0.005 && peak > 0.02 {
            speech_windows += 1;
        }
    }

    let has_speech = speech_windows > 0;

    app_log!("{} audio: max_rms={:.6}, max_peak={:.4}, speech_windows={}/{}, speech={}",
        speaker, max_rms, max_peak, speech_windows, total_windows, has_speech);

    if !has_speech {
        app_log!("{}: ingen tale detekteret, springer over", speaker);
    }

    has_speech
}

fn load_wav_mono_f32(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).context("Kunne ikke åbne WAV-fil")?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
    };

    if spec.channels == 2 {
        Ok(samples
            .chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    (chunk[0] + chunk[1]) / 2.0
                } else {
                    chunk[0]
                }
            })
            .collect())
    } else {
        Ok(samples)
    }
}
