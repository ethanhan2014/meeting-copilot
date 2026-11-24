use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;

pub const CHUNK_BYTES: usize = 960; // 20 ms, 24 kHz mono PCM16.

#[cfg(any(target_os = "macos", test))]
#[path = "capture_macos.rs"]
mod macos;

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    Meeting,
    Microphone,
}

impl AudioSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Meeting => "Meeting audio",
            Self::Microphone => "Your microphone",
        }
    }
}

#[derive(Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct AudioDevices {
    pub outputs: Vec<AudioDevice>,
    pub microphones: Vec<AudioDevice>,
}

pub struct Capture {
    pub audio: mpsc::Receiver<Result<Vec<u8>, String>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.audio.close();
        if let Some(thread) = self.thread.take() {
            // macOS permission dialogs and native completions may outlive Stop; the worker cleans up.
            #[cfg(target_os = "macos")]
            if !thread.is_finished() {
                return;
            }
            let _ = thread.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
pub fn devices() -> Result<AudioDevices, String> {
    std::thread::spawn(|| {
        use wasapi::*;
        initialize_mta().ok().map_err(|e| e.to_string())?;
        let result = (|| {
            let enumerator = DeviceEnumerator::new()?;
            let read = |direction| -> Result<Vec<AudioDevice>, wasapi::WasapiError> {
                let collection = enumerator.get_device_collection(direction)?;
                let mut devices = Vec::new();
                for i in 0..collection.get_nbr_devices()? {
                    let device = collection.get_device_at_index(i)?;
                    devices.push(AudioDevice {
                        id: device.get_id()?,
                        name: device.get_friendlyname()?,
                    });
                }
                Ok(devices)
            };
            Ok::<_, wasapi::WasapiError>(AudioDevices {
                outputs: read(Direction::Render)?,
                microphones: read(Direction::Capture)?,
            })
        })()
        .map_err(|e| e.to_string());
        deinitialize();
        result
    })
    .join()
    .map_err(|_| "Audio device enumeration failed.".to_string())?
}

#[cfg(target_os = "macos")]
pub fn devices() -> Result<AudioDevices, String> {
    macos::devices()
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn devices() -> Result<AudioDevices, String> {
    Err("Audio capture requires the Windows or macOS 15+ app.".into())
}

pub fn start(device_id: String, source: AudioSource) -> Capture {
    let (tx, audio) = mpsc::channel(100);
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let thread = std::thread::spawn(move || {
        if let Err(error) = capture_loop(device_id, source, &tx, &stopped) {
            #[cfg(target_os = "macos")]
            let _ = tx.blocking_send(Err(error));
            #[cfg(not(target_os = "macos"))]
            let _ = tx.try_send(Err(error));
        }
    });
    Capture {
        audio,
        stop,
        thread: Some(thread),
    }
}

#[cfg(windows)]
fn capture_loop(
    device_id: String,
    source: AudioSource,
    tx: &mpsc::Sender<Result<Vec<u8>, String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };
    use wasapi::*;
    initialize_mta().ok().map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let enumerator = DeviceEnumerator::new()?;
        let direction = match source {
            AudioSource::Meeting => Direction::Render,
            AudioSource::Microphone => Direction::Capture,
        };
        let device = if device_id.is_empty() {
            enumerator.get_default_device(direction)?
        } else {
            enumerator.get_device(&device_id)?
        };
        let mut client = device.get_iaudioclient()?;
        let format = WaveFormat::new(16, 16, SampleType::Int, 24000, 1, None);
        client.initialize_client(
            &format,
            Direction::Capture,
            StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 1_000_000,
            },
            None,
        )?;
        let capture = client.get_audiocaptureclient()?;
        let mut queue = VecDeque::new();
        client.start_stream()?;
        let stream_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut next = Instant::now() + Duration::from_millis(20);
            while !stop.load(Ordering::Acquire) && !tx.is_closed() {
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
                next += Duration::from_millis(20);
                let mut received = false;
                while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                    capture.read_from_device_to_deque(&mut queue)?;
                    received = true;
                }
                while queue.len() >= CHUNK_BYTES {
                    tx.try_send(Ok(queue.drain(..CHUNK_BYTES).collect()))?;
                }
                // Idle render devices produce no packets: send silence so server VAD can finish a turn.
                if !received {
                    let mut chunk: Vec<u8> = queue.drain(..).collect();
                    chunk.resize(CHUNK_BYTES, 0);
                    tx.try_send(Ok(chunk))?;
                }
            }
            Ok(())
        })();
        let stop_result = client.stop_stream();
        stream_result?;
        stop_result?;
        Ok(())
    })()
    .map_err(|e| {
        format!(
            "{} capture stopped: {e}. Check the selected device and Windows audio permissions.",
            source.label()
        )
    });
    deinitialize();
    result
}

#[cfg(target_os = "macos")]
fn capture_loop(
    device_id: String,
    source: AudioSource,
    tx: &mpsc::Sender<Result<Vec<u8>, String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    macos::capture_loop(device_id, source, tx, stop)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn capture_loop(
    _: String,
    _: AudioSource,
    _: &mpsc::Sender<Result<Vec<u8>, String>>,
    _: &AtomicBool,
) -> Result<(), String> {
    Err("Audio capture requires the Windows or macOS 15+ app.".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn windows_capture_delivers_pcm_and_stops() {
        for source in [AudioSource::Meeting, AudioSource::Microphone] {
            let mut capture = start(String::new(), source);
            for _ in 0..10 {
                let chunk = tokio::time::timeout(Duration::from_secs(3), capture.audio.recv())
                    .await
                    .expect("Audio capture timed out")
                    .expect("Audio capture ended")
                    .expect("WASAPI capture failed");
                assert_eq!(chunk.len(), CHUNK_BYTES);
            }
            let before = Instant::now();
            drop(capture);
            assert!(
                before.elapsed() < Duration::from_secs(1),
                "Capture did not stop promptly"
            );
        }
    }
}
