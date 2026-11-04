//! cpal audio output for media playback: the device pulls mixed samples from the
//! `deckcraft_media::Player`, which makes it the playback clock video frames follow.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use deckcraft_media::{AudioOut, CaptureError, DeviceInfo};

/// The first stream error since it was last reported. cpal calls the error callback on its
/// realtime audio thread (on ALSA in a loop while the device stays broken), which must not log:
/// the callback only keeps the first error, and the UI thread logs it when the stream stops.
type ErrorSlot = Arc<Mutex<Option<String>>>;

/// Keep `e` unless an error is already kept. Never blocks: a busy slot drops the error.
fn keep_stream_error(slot: &ErrorSlot, e: &dyn std::fmt::Display) {
    if let Ok(mut kept) = slot.try_lock()
        && kept.is_none()
    {
        *kept = Some(e.to_string());
    }
}

/// The kept error, once (a poisoned lock is tolerated).
fn take_stream_error(slot: &ErrorSlot) -> Option<String> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

#[derive(Default)]
pub struct CpalOut {
    stream: Option<cpal::Stream>,
    error: ErrorSlot,
    /// Set by the realtime error callback when the device went away: the stream is dead.
    dead: Arc<AtomicBool>,
}

impl CpalOut {
    /// Log the stream error the realtime callback kept (UI thread).
    fn report(&self) {
        if let Some(e) = take_stream_error(&self.error) {
            log::warn!("audio stream error: {e}");
        }
    }
}

impl Drop for CpalOut {
    fn drop(&mut self) {
        self.stream = None;
        self.report();
    }
}

/// Mono samples from an interleaved frame. Stops at `cap` samples.
fn push_mono(out: &mut Vec<f32>, interleaved: &[f32], channels: usize, cap: usize) {
    let ch = channels.max(1);
    let frames = interleaved.len() / ch;
    let room = cap.saturating_sub(out.len());
    for i in 0..frames.min(room) {
        let mut sum = 0.0f32;
        let mut n = 0u32;
        for c in 0..ch {
            if let Some(s) = interleaved.get(i * ch + c)
                && s.is_finite()
            {
                sum += *s;
                n += 1;
            }
        }
        if n > 0 {
            out.push((sum / n as f32).clamp(-1.0, 1.0));
        }
    }
}

/// Ten minutes of mono audio, so one slide cannot grow without bound.
const MAX_SAMPLES: usize = 48_000 * 60 * 10;

#[derive(Default)]
struct Capture {
    rate: u32,
    samples: Vec<f32>,
}

/// Default input device. The callback appends mono f32; the UI thread drains it.
pub struct CpalIn {
    stream: Option<cpal::Stream>,
    buf: Arc<Mutex<Capture>>,
    error: ErrorSlot,
    /// `DeviceInfo::id` from the Record view's input menu; `None` is the system default.
    selected: Option<String>,
}

impl Default for CpalIn {
    fn default() -> Self {
        CpalIn { stream: None, buf: Arc::new(Mutex::new(Capture::default())), error: ErrorSlot::default(), selected: None }
    }
}

/// Resolve a `DeviceInfo::id` from [`CpalIn::enumerate`] to a cpal input device. The id carries
/// the enumeration index so two inputs with the same name stay distinct.
fn find_input(host: &cpal::Host, id: &str) -> Result<cpal::Device, CaptureError> {
    let index: usize = id.strip_prefix("input:").and_then(|n| n.parse().ok()).ok_or(CaptureError::Unavailable)?;
    host.input_devices().map_err(|e| CaptureError::Failed(e.to_string()))?.nth(index).ok_or(CaptureError::Unavailable)
}

/// Platform wording for a refusal or contention, so the device control can show Denied or Busy
/// instead of a generic failure.
fn classify_capture_error(text: &str) -> CaptureError {
    let lower = text.to_ascii_lowercase();
    if lower.contains("denied") || lower.contains("permission") || lower.contains("privacy") || lower.contains("not authorized") {
        CaptureError::Denied
    } else if lower.contains("busy") || lower.contains("in use") || lower.contains("exclusive") {
        CaptureError::Busy
    } else if lower.contains("no microphone") || lower.contains("no input") || lower.contains("no device") || lower.contains("not found") {
        CaptureError::Unavailable
    } else {
        CaptureError::Failed(text.to_string())
    }
}

impl CpalIn {
    fn report(&self) {
        if let Some(e) = take_stream_error(&self.error) {
            log::warn!("microphone stream error: {e}");
        }
    }
}

impl Drop for CpalIn {
    fn drop(&mut self) {
        self.stream = None;
        self.report();
    }
}

impl deckcraft_media::MicIn for CpalIn {
    fn start(&mut self) -> Result<(), String> {
        self.stop();
        let host = cpal::default_host();
        let dev = match &self.selected {
            Some(id) => find_input(&host, id).map_err(|e| e.message("microphone"))?,
            None => host.default_input_device().ok_or_else(|| "no microphone".to_string())?,
        };
        let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
        let channels = cfg.channels() as usize;
        let rate = cfg.sample_rate().0;
        if !(8_000..=192_000).contains(&rate) {
            return Err(format!("microphone sample rate {rate} is not supported"));
        }
        {
            let mut g = self.buf.lock().unwrap_or_else(PoisonError::into_inner);
            g.rate = rate;
            g.samples.clear();
        }
        let config: cpal::StreamConfig = cfg.clone().into();
        let buf = self.buf.clone();
        let slot = self.error.clone();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => {
                let err = move |e: cpal::StreamError| keep_stream_error(&slot, &e);
                dev.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        if let Ok(mut g) = buf.try_lock() {
                            push_mono(&mut g.samples, data, channels.max(1), MAX_SAMPLES);
                        }
                    },
                    err,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let err = move |e: cpal::StreamError| keep_stream_error(&slot, &e);
                dev.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        if let Ok(mut g) = buf.try_lock() {
                            let mut tmp = Vec::with_capacity(data.len());
                            for s in data {
                                tmp.push(*s as f32 / i16::MAX as f32);
                            }
                            push_mono(&mut g.samples, &tmp, channels.max(1), MAX_SAMPLES);
                        }
                    },
                    err,
                    None,
                )
            }
            other => return Err(format!("unsupported microphone format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    fn take(&mut self) -> (u32, Vec<f32>) {
        let mut g = self.buf.lock().unwrap_or_else(PoisonError::into_inner);
        let rate = g.rate;
        let samples = std::mem::take(&mut g.samples);
        (rate, samples)
    }

    fn captured(&self) -> usize {
        self.buf.lock().unwrap_or_else(PoisonError::into_inner).samples.len()
    }

    fn stop(&mut self) {
        self.stream = None;
        self.report();
        self.buf.lock().unwrap_or_else(PoisonError::into_inner).samples.clear();
    }

    fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, CaptureError> {
        let host = cpal::default_host();
        let devices = host.input_devices().map_err(|e| CaptureError::Failed(e.to_string()))?;
        let mut out = Vec::new();
        for (i, dev) in devices.enumerate() {
            let name = dev.name().unwrap_or_else(|_| format!("Input {}", i + 1));
            out.push(DeviceInfo::new(format!("input:{i}"), name));
        }
        Ok(out)
    }

    fn select(&mut self, id: Option<&str>) -> Result<(), CaptureError> {
        let Some(id) = id else {
            self.selected = None;
            return Ok(());
        };
        let host = cpal::default_host();
        find_input(&host, id)?;
        self.selected = Some(id.to_string());
        Ok(())
    }

    fn level(&mut self) -> f32 {
        let g = self.buf.lock().unwrap_or_else(PoisonError::into_inner);
        g.samples.iter().rev().take(2048).fold(0.0f32, |peak, s| peak.max(s.abs())).clamp(0.0, 1.0)
    }

    fn failure(&mut self) -> Option<CaptureError> {
        take_stream_error(&self.error).map(|e| classify_capture_error(&e))
    }

    fn running(&self) -> bool {
        self.stream.is_some()
    }
}

impl AudioOut for CpalOut {
    fn start(&mut self, mut fill: Box<dyn FnMut(&mut [f32], usize, u32) + Send>) -> Result<(), String> {
        self.stop();
        let dev = cpal::default_host().default_output_device().ok_or("no audio output device")?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        let channels = cfg.channels() as usize;
        let rate = cfg.sample_rate().0;
        let config: cpal::StreamConfig = cfg.clone().into();
        let slot = self.error.clone();
        let dead = Arc::new(AtomicBool::new(false));
        self.dead = dead.clone();
        let err = move |e: cpal::StreamError| {
            if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                dead.store(true, Ordering::Relaxed);
            }
            keep_stream_error(&slot, &e);
        };
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => dev.build_output_stream(&config, move |buf: &mut [f32], _| fill(buf, channels, rate), err, None),
            cpal::SampleFormat::I16 => {
                let mut tmp = Vec::new();
                dev.build_output_stream(
                    &config,
                    move |buf: &mut [i16], _| {
                        tmp.resize(buf.len(), 0.0);
                        fill(&mut tmp, channels, rate);
                        for (o, s) in buf.iter_mut().zip(&tmp) {
                            *o = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    },
                    err,
                    None,
                )
            }
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) {
        self.stream = None;
        self.report();
    }

    fn failure(&mut self) -> Option<String> {
        if !self.dead.swap(false, Ordering::Relaxed) {
            return None;
        }
        Some(take_stream_error(&self.error).unwrap_or_else(|| "the audio device is no longer available".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_stream_error_is_kept_and_it_is_taken_once() {
        let slot = ErrorSlot::default();
        assert_eq!(take_stream_error(&slot), None);
        keep_stream_error(&slot, &"device unplugged");
        keep_stream_error(&slot, &"device unplugged again");
        assert_eq!(take_stream_error(&slot).as_deref(), Some("device unplugged"));
        assert_eq!(take_stream_error(&slot), None);
        keep_stream_error(&slot, &"a later error");
        assert_eq!(take_stream_error(&slot).as_deref(), Some("a later error"));
    }

    #[test]
    fn a_poisoned_slot_still_reports() {
        let slot = ErrorSlot::default();
        keep_stream_error(&slot, &"lost");
        let s = slot.clone();
        let _ = std::thread::spawn(move || {
            let _g = s.lock();
            panic!("poison the slot");
        })
        .join();
        assert!(slot.is_poisoned());
        assert_eq!(take_stream_error(&slot).as_deref(), Some("lost"));
    }

    #[test]
    fn mono_downmix_averages_channels_and_stops_at_the_cap() {
        let mut out = Vec::new();
        push_mono(&mut out, &[0.5, -0.5, 1.0, 1.0], 2, 10);
        assert_eq!(out, vec![0.0, 1.0]);
        push_mono(&mut out, &[f32::NAN, 0.5], 2, 10);
        assert_eq!(out.len(), 3);
        assert!((out[2] - 0.5).abs() < 1e-6);
        let before = out.len();
        push_mono(&mut out, &[0.25], 1, 3);
        assert_eq!(out.len(), before);
    }
}
