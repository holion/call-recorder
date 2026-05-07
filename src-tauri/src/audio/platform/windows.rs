use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::{Arc, Mutex};

use super::SystemAudioCaptureTrait;

pub struct WindowsSystemAudioCapture {
    stream: Option<Stream>,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl WindowsSystemAudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();

        // On Windows, we can open the default output device as an input (WASAPI loopback)
        let device = host
            .default_output_device()
            .context("Ingen lydoutput-enhed fundet")?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;

        let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let samples_clone = samples.clone();

        // Build input stream on output device = WASAPI loopback capture
        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut buf) = samples_clone.lock() {
                    buf.extend_from_slice(data);
                }
            },
            |err| eprintln!("System audio fejl: {}", err),
            None,
        )?;

        Ok(Self {
            stream: Some(stream),
            samples,
            sample_rate,
        })
    }
}

impl SystemAudioCaptureTrait for WindowsSystemAudioCapture {
    fn start(&self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream.play()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream.pause()?;
        }
        Ok(())
    }

    fn take_samples(&self) -> Vec<f32> {
        if let Ok(mut buf) = self.samples.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        }
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn samples_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.samples.clone()
    }
}

unsafe impl Send for WindowsSystemAudioCapture {}
