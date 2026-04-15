use anyhow::{Context, Result};
use screencapturekit::cm::CMSampleBuffer;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
use screencapturekit::stream::output_type::SCStreamOutputType;
use screencapturekit::stream::SCStream;
use std::sync::{Arc, Mutex};

use super::SystemAudioCaptureTrait;

struct AudioHandler {
    samples: Arc<Mutex<Vec<f32>>>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample_buffer: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type == SCStreamOutputType::Audio {
            if let Some(audio_buffer_list) = sample_buffer.audio_buffer_list() {
                if let Ok(mut buf) = self.samples.lock() {
                    // ScreenCaptureKit delivers planar audio: each buffer in the
                    // list is a separate channel (L, R, …). We only need one
                    // channel for mono output, so take the first buffer only.
                    let mut first = true;
                    for audio_buffer in &audio_buffer_list {
                        if !first {
                            break;
                        }
                        first = false;
                        let data = audio_buffer.data();
                        let float_samples: &[f32] = unsafe {
                            std::slice::from_raw_parts(
                                data.as_ptr() as *const f32,
                                data.len() / std::mem::size_of::<f32>(),
                            )
                        };
                        buf.extend_from_slice(float_samples);
                    }
                }
            }
        }
    }
}

pub struct MacOSSystemAudioCapture {
    stream: SCStream,
    samples: Arc<Mutex<Vec<f32>>>,
}

impl MacOSSystemAudioCapture {
    pub fn new() -> Result<Self> {
        let content =
            SCShareableContent::get().map_err(|e| anyhow::anyhow!("SCShareableContent fejl: {}", e))?;

        let displays = content.displays();
        let display = displays
            .first()
            .context("Ingen skærm fundet")?;

        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();

        // ScreenCaptureKit always captures video — use minimal settings to reduce overhead
        let config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_fps(1)
            .with_shows_cursor(false)
            .with_captures_audio(true)
            .with_excludes_current_process_audio(true);

        let samples = Arc::new(Mutex::new(Vec::new()));

        let handler = AudioHandler {
            samples: samples.clone(),
        };

        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(handler, SCStreamOutputType::Audio);

        Ok(Self { stream, samples })
    }
}

impl SystemAudioCaptureTrait for MacOSSystemAudioCapture {
    fn start(&self) -> Result<()> {
        self.stream
            .start_capture()
            .map_err(|e| anyhow::anyhow!("Kunne ikke starte systemlydoptagelse: {}", e))
    }

    fn stop(&mut self) -> Result<()> {
        self.stream
            .stop_capture()
            .map_err(|e| anyhow::anyhow!("Kunne ikke stoppe systemlydoptagelse: {}", e))
    }

    fn take_samples(&self) -> Vec<f32> {
        if let Ok(mut buf) = self.samples.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        }
    }

    fn sample_rate(&self) -> u32 {
        48000 // Default macOS ScreenCaptureKit sample rate
    }

    fn samples_arc(&self) -> std::sync::Arc<std::sync::Mutex<Vec<f32>>> {
        self.samples.clone()
    }
}

unsafe impl Send for MacOSSystemAudioCapture {}
