use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::{Arc, Mutex};

pub struct MicCapture {
    stream: Option<Stream>,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl MicCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("Ingen mikrofon fundet")?;

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels();

        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let samples_clone = samples.clone();

        let ch = channels;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = to_mono(data, ch);
                    if let Ok(mut buf) = samples_clone.lock() {
                        buf.extend_from_slice(&mono);
                    }
                },
                |err| eprintln!("Mikrofon fejl: {}", err),
                None,
            )?,
            cpal::SampleFormat::I16 => {
                let samples_clone = samples.clone();
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let floats: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        let mono = to_mono(&floats, ch);
                        if let Ok(mut buf) = samples_clone.lock() {
                            buf.extend_from_slice(&mono);
                        }
                    },
                    |err| eprintln!("Mikrofon fejl: {}", err),
                    None,
                )?
            }
            format => anyhow::bail!("Ikke-understøttet sample format: {:?}", format),
        };

        Ok(Self {
            stream: Some(stream),
            samples,
            sample_rate,
        })
    }

    pub fn start(&self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream.play()?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream.pause()?;
        }
        Ok(())
    }

    pub fn take_samples(&self) -> Vec<f32> {
        if let Ok(mut buf) = self.samples.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn samples_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.samples.clone()
    }
}

// Safety: Stream is Send on macOS/Windows
unsafe impl Send for MicCapture {}

fn to_mono(data: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    let ch = channels as usize;
    data.chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}
