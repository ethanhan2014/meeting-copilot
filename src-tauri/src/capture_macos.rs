use super::CHUNK_BYTES;
use std::collections::VecDeque;

#[derive(Default)]
struct PcmQueue {
    bytes: VecDeque<u8>,
    rate: f64,
    frame: u64,
    next: f64,
    previous: f32,
}

impl PcmQueue {
    fn push(&mut self, samples: &[f32], rate: f64) -> Result<(), String> {
        if !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) {
            return Err("Unsupported capture sample rate.".into());
        }
        if self.rate != rate {
            self.rate = rate;
            self.frame = 0;
            self.next = 0.0;
        }
        // shortcut: linear resampling targets speech; use a filtered resampler for high-fidelity audio.
        for &sample in samples {
            let sample = if sample.is_finite() { sample } else { 0.0 };
            while self.next <= self.frame as f64 {
                let value = if self.frame == 0 {
                    sample
                } else {
                    let fraction = (self.next - (self.frame - 1) as f64) as f32;
                    self.previous + (sample - self.previous) * fraction
                };
                let pcm = (value.clamp(-1.0, 1.0) * 32768.0).round() as i16;
                self.bytes.extend(pcm.to_le_bytes());
                self.next += rate / 24000.0;
            }
            self.previous = sample;
            self.frame += 1;
        }
        if self.bytes.len() > CHUNK_BYTES * 50 {
            return Err("Audio capture fell behind. Restart recording.".into());
        }
        Ok(())
    }

    fn chunk(&mut self) -> Vec<u8> {
        let mut chunk: Vec<u8> = self
            .bytes
            .drain(..self.bytes.len().min(CHUNK_BYTES))
            .collect();
        // Silence maintains the real-time clock and lets server VAD finish a turn.
        chunk.resize(CHUNK_BYTES, 0);
        chunk
    }
}

fn mono_float_pcm(buffers: &[(&[u8], usize)], channels: usize) -> Result<Vec<f32>, String> {
    if channels == 0 || channels > 32 || buffers.iter().map(|(_, n)| n).sum::<usize>() != channels {
        return Err("Invalid capture audio channel layout.".into());
    }
    let mut frames = None;
    for &(data, count) in buffers {
        if count == 0 || data.len() % (count * 4) != 0 {
            return Err("Invalid capture audio buffer length.".into());
        }
        let count = data.len() / (count * 4);
        if frames.is_some_and(|frames| frames != count) {
            return Err("Capture audio channels have different lengths.".into());
        }
        frames = Some(count);
    }
    let mut mono = vec![0.0; frames.unwrap_or(0)];
    for &(data, count) in buffers {
        for (frame, bytes) in data.chunks_exact(count * 4).enumerate() {
            for bytes in bytes.chunks_exact(4) {
                let value = f32::from_le_bytes(bytes.try_into().unwrap());
                if value.is_finite() {
                    mono[frame] += value / channels as f32;
                }
            }
        }
    }
    Ok(mono)
}

#[cfg(target_os = "macos")]
mod native {
    use super::{mono_float_pcm, PcmQueue};
    use crate::capture::{AudioDevice, AudioDevices, AudioSource};
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use screencapturekit::{
        audio_devices::AudioInputDevice, prelude::*, stream::delegate_trait::ErrorHandler,
    };
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };
    use tokio::sync::mpsc;

    pub fn devices() -> Result<AudioDevices, String> {
        Ok(AudioDevices {
            outputs: vec![AudioDevice {
                id: "system-audio".into(),
                name: "System audio (all applications)".into(),
            }],
            microphones: AudioInputDevice::list()
                .into_iter()
                .map(|device| AudioDevice {
                    id: device.id,
                    name: device.name,
                })
                .collect(),
        })
    }

    fn microphone_permission(stop: &AtomicBool) -> Result<bool, String> {
        // AVMediaTypeAudio is a valid constant and the block is copied by AVFoundation.
        let media_type = unsafe { AVMediaTypeAudio }.ok_or("Microphone access is unavailable.")?;
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
        if status == AVAuthorizationStatus::Authorized {
            return Ok(true);
        }
        if status != AVAuthorizationStatus::NotDetermined {
            return Err("Microphone permission denied. Enable Meeting Copilot in System Settings → Privacy & Security → Microphone, then restart the app.".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let completion = RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &completion);
        }
        while !stop.load(Ordering::Acquire) {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(true) => return Ok(true),
                Ok(false) => return Err("Microphone permission denied. Enable Meeting Copilot in System Settings → Privacy & Security → Microphone, then restart the app.".into()),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
                Err(_) => return Err("Microphone permission request failed.".into()),
            }
        }
        Ok(false)
    }

    #[derive(Default)]
    struct AudioState {
        pcm: PcmQueue,
        error: Option<String>,
    }

    fn append_sample(state: &mut AudioState, sample: CMSampleBuffer) -> Result<(), String> {
        if !sample.is_valid() || !sample.data_is_ready() {
            return Err("ScreenCaptureKit delivered an invalid audio sample.".into());
        }
        let format = sample
            .format_description()
            .ok_or("Capture audio format is missing.")?;
        if !format.is_pcm()
            || !format.audio_is_float()
            || format.audio_is_big_endian()
            || format.audio_bits_per_channel() != Some(32)
        {
            return Err("ScreenCaptureKit audio must be little-endian float32 PCM.".into());
        }
        let rate = format
            .audio_sample_rate()
            .ok_or("Capture audio sample rate is missing.")?;
        let channels = format
            .audio_channel_count()
            .ok_or("Capture audio channel count is missing.")? as usize;
        let list = sample
            .audio_buffer_list()
            .map_err(|code| format!("Unable to read capture audio (CoreMedia {code})."))?;
        let buffers: Vec<_> = list
            .iter()
            .map(|buffer| (buffer.data(), buffer.number_channels as usize))
            .collect();
        let mono = mono_float_pcm(&buffers, channels)?;
        state.pcm.push(&mono, rate)
    }

    pub fn capture_loop(
        device_id: String,
        source: AudioSource,
        tx: &mpsc::Sender<Result<Vec<u8>, String>>,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let capture = || -> Result<(), String> {
            if stop.load(Ordering::Acquire) || tx.is_closed() {
                return Ok(());
            }
            match source {
                AudioSource::Meeting if !device_id.is_empty() && device_id != "system-audio" => {
                    return Err("Select System audio for macOS meeting capture.".into())
                }
                AudioSource::Microphone => {
                    if !microphone_permission(stop)? {
                        return Ok(());
                    }
                    if !device_id.is_empty()
                        && !AudioInputDevice::list()
                            .iter()
                            .any(|device| device.id == device_id)
                    {
                        return Err("The selected microphone is unavailable. Select an available microphone.".into());
                    }
                }
                _ => {}
            }
            let content = SCShareableContent::get().map_err(|e| format!("{e}. Enable Meeting Copilot in System Settings → Privacy & Security → Screen & System Audio Recording, then restart the app."))?;
            if stop.load(Ordering::Acquire) || tx.is_closed() {
                return Ok(());
            }
            let display = content
                .displays()
                .into_iter()
                .next()
                .ok_or("No display is available for system audio capture.")?;
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&[])
                .build()
                .map_err(|e| e.to_string())?;
            let mut config = SCStreamConfiguration::new()
                .with_width(2)
                .with_height(2)
                .with_fps(1)
                .with_sample_rate(24000)
                .with_channel_count(1)
                .with_excludes_current_process_audio(true)
                .with_captures_audio(source == AudioSource::Meeting)
                .with_captures_microphone(source == AudioSource::Microphone)
                .map_err(|e| e.to_string())?;
            if source == AudioSource::Microphone && !device_id.is_empty() {
                config
                    .set_microphone_capture_device_id(&device_id)
                    .map_err(|e| e.to_string())?;
            }
            let state = Arc::new(Mutex::new(AudioState::default()));
            let errors = state.clone();
            let delegate = ErrorHandler::new(move |error| {
                if let Ok(mut state) = errors.lock() {
                    state.error.get_or_insert_with(|| error.to_string());
                }
            });
            let mut stream = SCStream::new_with_delegate(&filter, &config, delegate)
                .map_err(|e| e.to_string())?;
            let output = if source == AudioSource::Meeting {
                SCStreamOutputType::Audio
            } else {
                SCStreamOutputType::Microphone
            };
            let audio = state.clone();
            stream
                .add_output_handler(
                    move |sample: CMSampleBuffer, _: SCStreamOutputType| {
                        if let Ok(mut state) = audio.lock() {
                            if state.error.is_none() {
                                if let Err(error) = append_sample(&mut state, sample) {
                                    state.error = Some(error);
                                }
                            }
                        }
                    },
                    output,
                )
                .map_err(|e| e.to_string())?;
            if stop.load(Ordering::Acquire) || tx.is_closed() {
                return Ok(());
            }
            stream.start_capture().map_err(|e| e.to_string())?;
            let result = (|| {
                let mut next = Instant::now() + Duration::from_millis(20);
                while !stop.load(Ordering::Acquire) && !tx.is_closed() {
                    std::thread::sleep(next.saturating_duration_since(Instant::now()));
                    next += Duration::from_millis(20);
                    let mut state = state.lock().map_err(|_| "Audio capture state failed.")?;
                    if let Some(error) = state.error.take() {
                        return Err(error);
                    }
                    match tx.try_send(Ok(state.pcm.chunk())) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            return Err("Audio processing fell behind. Restart recording.".into())
                        }
                    }
                }
                Ok(())
            })();
            let stopped = stream.stop_capture().map_err(|e| e.to_string());
            result?;
            stopped
        };
        capture().map_err(|e| format!("{} capture stopped: {e}. Check macOS Screen & System Audio Recording and Microphone permissions.", source.label()))
    }
}

#[cfg(target_os = "macos")]
pub use native::{capture_loop, devices};

#[cfg(test)]
mod tests {
    use super::*;

    fn floats(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn float_pcm_clips_and_silences_nonfinite_samples() {
        let mut pcm = PcmQueue::default();
        pcm.push(&[-2.0, -0.5, 0.5, 2.0, f32::NAN, f32::INFINITY], 24000.0)
            .unwrap();
        let chunk = pcm.chunk();
        let values: Vec<_> = chunk[..12]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(values, [-32768, -16384, 16384, 32767, 0, 0]);
        assert_eq!(chunk.len(), CHUNK_BYTES);
        assert!(chunk[12..].iter().all(|&byte| byte == 0));
        assert!(pcm.chunk().iter().all(|&byte| byte == 0));
    }

    #[test]
    fn downmixes_interleaved_and_planar_audio_identically() {
        let interleaved = floats(&[0.75, 0.25, -0.5, 0.5]);
        let left = floats(&[0.75, -0.5]);
        let right = floats(&[0.25, 0.5]);
        assert_eq!(mono_float_pcm(&[(&interleaved, 2)], 2).unwrap(), [0.5, 0.0]);
        assert_eq!(
            mono_float_pcm(&[(&left, 1), (&right, 1)], 2).unwrap(),
            [0.5, 0.0]
        );
        assert!(mono_float_pcm(&[(&left[..3], 1)], 1).is_err());
        assert!(mono_float_pcm(&[(&left, 1)], 2).is_err());
        assert!(mono_float_pcm(&[(&left, 1), (&right[..4], 1)], 2).is_err());
    }

    #[test]
    fn resampling_preserves_phase_across_callback_boundaries() {
        for rate in [16000.0, 44100.0, 48000.0] {
            let samples: Vec<_> = (0..960).map(|n| (n as f32 * 0.01).sin()).collect();
            let mut whole = PcmQueue::default();
            whole.push(&samples, rate).unwrap();
            let mut split = PcmQueue::default();
            for samples in samples.chunks(137) {
                split.push(samples, rate).unwrap();
            }
            assert_eq!(whole.bytes, split.bytes);
            assert_eq!(
                whole.bytes.len() / 2,
                ((samples.len() - 1) as f64 * 24000.0 / rate).floor() as usize + 1
            );
        }
    }

    #[test]
    fn chunks_keep_audio_order_and_bound_backlog() {
        let mut pcm = PcmQueue::default();
        pcm.push(&vec![0.5; 720], 24000.0).unwrap();
        assert_eq!(pcm.chunk(), [0, 64].repeat(480));
        let tail = pcm.chunk();
        assert_eq!(&tail[..480], [0, 64].repeat(240));
        assert!(tail[480..].iter().all(|&byte| byte == 0));
        assert!(pcm.push(&vec![0.0; 24001], 24000.0).is_err());
        assert!(pcm.push(&[], f64::NAN).is_err());
    }
}
