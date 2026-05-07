use crate::app_log;
use crate::transcription::whisper::{
    contains_speech, contains_speech_for_dictation, load_wav_mono_f32, TimedSegment,
};
use anyhow::{anyhow, Context, Result};
use hound::{WavSpec, WavWriter};
use reqwest::blocking::multipart::{Form, Part};
use serde::Deserialize;
use std::io::Cursor;
use std::path::Path;

const OPENAI_TRANSCRIPTION_MODEL: &str = "whisper-1";
const TARGET_SAMPLE_RATE: u32 = 16000;
// 10 minutes of 16 kHz mono WAV is safely below OpenAI's 25 MB upload limit.
const CHUNK_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 60 * 10;

#[derive(Debug, Deserialize)]
struct OpenAiTranscription {
    text: String,
    duration: Option<f64>,
    segments: Option<Vec<OpenAiSegment>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiSegment {
    start: f64,
    end: f64,
    text: String,
}

pub fn transcribe_channel(
    api_key: &str,
    wav_path: &Path,
    speaker: &str,
) -> Result<Vec<TimedSegment>> {
    app_log!(
        "[openai-transcription] Transskriberer {} via OpenAI...",
        speaker
    );
    let audio = load_wav_mono_f32(wav_path)?;
    transcribe_audio(api_key, &audio, speaker)
}

pub fn transcribe_audio(api_key: &str, audio: &[f32], speaker: &str) -> Result<Vec<TimedSegment>> {
    if audio.is_empty() {
        return Ok(Vec::new());
    }

    let mut all = Vec::new();
    for (chunk_index, chunk) in audio.chunks(CHUNK_SAMPLES).enumerate() {
        let has_speech = if speaker == "Diktation" {
            contains_speech_for_dictation(chunk, speaker)
        } else {
            contains_speech(chunk, speaker)
        };

        if !has_speech {
            continue;
        }

        let offset_ms = (chunk_index * CHUNK_SAMPLES) as i64 * 1000 / TARGET_SAMPLE_RATE as i64;
        let mut segments = transcribe_chunk(api_key, chunk, speaker, offset_ms)
            .with_context(|| format!("OpenAI-transskription fejlede for {}", speaker))?;
        all.append(&mut segments);
    }

    app_log!(
        "[openai-transcription] {}: {} segmenter fundet",
        speaker,
        all.len()
    );
    Ok(all)
}

fn transcribe_chunk(
    api_key: &str,
    audio: &[f32],
    speaker: &str,
    offset_ms: i64,
) -> Result<Vec<TimedSegment>> {
    let wav = wav_bytes(audio)?;
    let part = Part::bytes(wav)
        .file_name(format!("{}.wav", speaker.to_lowercase()))
        .mime_str("audio/wav")?;

    let form = Form::new()
        .text("model", OPENAI_TRANSCRIPTION_MODEL)
        .text("language", "da")
        .text("response_format", "verbose_json")
        .part("file", part);

    let response = reqwest::blocking::Client::new()
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .context("Kunne ikke kontakte OpenAI transcription API")?;

    let status = response.status();
    let body = response
        .text()
        .context("Kunne ikke læse OpenAI transcription svar")?;

    if !status.is_success() {
        return Err(anyhow!(
            "OpenAI transcription API fejl {}: {}",
            status,
            body
        ));
    }

    let parsed: OpenAiTranscription =
        serde_json::from_str(&body).context("Kunne ikke parse OpenAI transcription svar")?;

    let segments = parsed.segments.unwrap_or_default();
    if segments.is_empty() {
        let text = parsed.text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let duration_ms = parsed
            .duration
            .map(|d| (d * 1000.0) as i64)
            .unwrap_or_else(|| audio.len() as i64 * 1000 / TARGET_SAMPLE_RATE as i64);

        return Ok(vec![TimedSegment {
            start_ms: offset_ms,
            end_ms: offset_ms + duration_ms,
            text: text.to_string(),
            speaker: speaker.to_string(),
        }]);
    }

    Ok(segments
        .into_iter()
        .filter_map(|segment| {
            let text = segment.text.trim().to_string();
            if text.is_empty() {
                return None;
            }

            Some(TimedSegment {
                start_ms: offset_ms + (segment.start * 1000.0) as i64,
                end_ms: offset_ms + (segment.end * 1000.0) as i64,
                text,
                speaker: speaker.to_string(),
            })
        })
        .collect())
}

fn wav_bytes(samples: &[f32]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let spec = WavSpec {
            channels: 1,
            sample_rate: TARGET_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            writer.write_sample((clamped * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}
