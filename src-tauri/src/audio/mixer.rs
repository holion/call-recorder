use anyhow::Result;
use hound::{WavSpec, WavWriter};
use std::path::Path;

const TARGET_SAMPLE_RATE: u32 = 16000;

/// Resample audio from source_rate to TARGET_SAMPLE_RATE using linear interpolation
pub fn resample(samples: &[f32], source_rate: u32) -> Vec<f32> {
    if source_rate == TARGET_SAMPLE_RATE {
        return samples.to_vec();
    }

    let ratio = source_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < samples.len() {
            samples[idx] as f64 * (1.0 - frac) + samples[idx + 1] as f64 * frac
        } else if idx < samples.len() {
            samples[idx] as f64
        } else {
            0.0
        };

        output.push(sample as f32);
    }

    output
}

fn save_wav(samples: &[f32], path: &Path) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        writer.write_sample((clamped * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Result of saving audio — contains paths and duration
pub struct SavedAudio {
    pub duration_secs: f64,
    pub has_system_audio: bool,
}

/// Save mic and system audio as separate WAVs + a mixed WAV for playback.
/// Files: {id}.wav (mixed), {id}_mic.wav, {id}_system.wav
pub fn save_separate_and_mixed(
    mic_samples: &[f32],
    mic_rate: u32,
    system_samples: &[f32],
    system_rate: u32,
    recordings_dir: &Path,
    id: &str,
) -> Result<SavedAudio> {
    let mic_resampled = resample(mic_samples, mic_rate);
    let sys_resampled = resample(system_samples, system_rate);

    let max_len = mic_resampled.len().max(sys_resampled.len());
    let duration_secs = max_len as f64 / TARGET_SAMPLE_RATE as f64;
    let has_system_audio = !sys_resampled.is_empty();

    // Save mic WAV
    if !mic_resampled.is_empty() {
        save_wav(&mic_resampled, &recordings_dir.join(format!("{}_mic.wav", id)))?;
    }

    // Save system WAV
    if has_system_audio {
        save_wav(&sys_resampled, &recordings_dir.join(format!("{}_system.wav", id)))?;
    }

    // Save mixed WAV for playback
    let mixed_path = recordings_dir.join(format!("{}.wav", id));
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(mixed_path, spec)?;
    for i in 0..max_len {
        let mic_val = mic_resampled.get(i).copied().unwrap_or(0.0);
        let sys_val = sys_resampled.get(i).copied().unwrap_or(0.0);
        let mixed = ((mic_val + sys_val) * 0.5).clamp(-1.0, 1.0);
        writer.write_sample((mixed * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;

    Ok(SavedAudio {
        duration_secs,
        has_system_audio,
    })
}
