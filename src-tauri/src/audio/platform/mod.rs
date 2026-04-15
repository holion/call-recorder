#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;
use std::sync::{Arc, Mutex};

pub trait SystemAudioCaptureTrait: Send {
    fn start(&self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn take_samples(&self) -> Vec<f32>;
    fn sample_rate(&self) -> u32;
    fn samples_arc(&self) -> Arc<Mutex<Vec<f32>>>;
}

pub type SystemAudioCaptureHandle = Box<dyn SystemAudioCaptureTrait>;

/// Dummy capture that returns no audio — used as fallback when system audio fails
struct DummyCapture;

impl SystemAudioCaptureTrait for DummyCapture {
    fn start(&self) -> Result<()> { Ok(()) }
    fn stop(&mut self) -> Result<()> { Ok(()) }
    fn take_samples(&self) -> Vec<f32> { Vec::new() }
    fn sample_rate(&self) -> u32 { 16000 }
    fn samples_arc(&self) -> Arc<Mutex<Vec<f32>>> { Arc::new(Mutex::new(Vec::new())) }
}

pub fn create_system_capture() -> Result<SystemAudioCaptureHandle> {
    #[cfg(target_os = "macos")]
    { Ok(Box::new(macos::MacOSSystemAudioCapture::new()?)) }

    #[cfg(target_os = "windows")]
    { Ok(Box::new(windows::WindowsSystemAudioCapture::new()?)) }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { Ok(Box::new(DummyCapture)) }
}

pub fn create_dummy_capture() -> SystemAudioCaptureHandle {
    Box::new(DummyCapture)
}
