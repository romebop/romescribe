use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;

enum AudioCommand {
    Start { ready: mpsc::Sender<Result<(), String>> },
    Stop { done: mpsc::Sender<()> },
    SetDevice { name: String, done: mpsc::Sender<Result<(), String>> },
}

/// Returns names of all available audio input devices.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Find an input device by name, or fall back to the default.
fn find_device(name: &str) -> Option<Device> {
    let host = cpal::default_host();
    if name.is_empty() {
        return host.default_input_device();
    }
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            if d.name().ok().as_deref() == Some(name) {
                return Some(d);
            }
        }
    }
    host.default_input_device()
}

/// Build a cpal input stream that pushes mono f32 samples into `buf`.
/// The stream is created in a paused state — call stream.play() to start.
fn build_stream(
    device: &Device,
    buf: &Arc<Mutex<Vec<f32>>>,
    rate: &Arc<Mutex<u32>>,
) -> Result<cpal::Stream, String> {
    let config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {}", e))?;

    *rate.lock().unwrap() = config.sample_rate().0;
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    println!(
        "[romescribe] Stream opened: {} (rate={}, channels={})",
        device.name().unwrap_or_default(),
        stream_config.sample_rate.0,
        channels
    );

    let err_fn = |err| eprintln!("[romescribe] Audio stream error: {}", err);

    let stream: cpal::Stream = match sample_format {
        SampleFormat::F32 => {
            let buf = buf.clone();
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut b = buf.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                            b.push(mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build stream: {}", e))?
        }
        SampleFormat::I16 => {
            let buf = buf.clone();
            device
                .build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mut b = buf.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk
                                .iter()
                                .map(|&s| s as f32 / 32768.0)
                                .sum::<f32>()
                                / channels as f32;
                            b.push(mono);
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Failed to build stream: {}", e))?
        }
        _ => return Err("Unsupported sample format".to_string()),
    };

    // Start paused — the stream holds the device connection open
    // but no audio callbacks fire until stream.play() is called
    stream
        .pause()
        .map_err(|e| format!("Failed to pause stream: {}", e))?;

    Ok(stream)
}

/// Thread-safe audio recorder. The audio stream stays open continuously
/// so Continuity Camera and Bluetooth devices don't need to reconnect
/// between recordings. The `recording` flag gates whether samples are
/// captured to the buffer.
pub struct AudioRecorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    sender: Mutex<mpsc::Sender<AudioCommand>>,
    sample_rate: Arc<Mutex<u32>>,
}

unsafe impl Send for AudioRecorder {}
unsafe impl Sync for AudioRecorder {}

impl AudioRecorder {
    pub fn new(device_name: &str) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::new(AtomicBool::new(false));
        let sample_rate = Arc::new(Mutex::new(0u32));
        let (tx, rx) = mpsc::channel::<AudioCommand>();

        let buf = buffer.clone();
        let rec = recording.clone();
        let rate = sample_rate.clone();
        let initial_device_name = device_name.to_string();

        thread::spawn(move || {
            // Open initial stream and keep it alive
            let device = match find_device(&initial_device_name) {
                Some(d) => d,
                None => {
                    eprintln!("[romescribe] No input device available");
                    return;
                }
            };

            let mut stream = match build_stream(&device, &buf, &rate) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[romescribe] Failed to open audio stream: {}", e);
                    return;
                }
            };

            while let Ok(cmd) = rx.recv() {
                match cmd {
                    AudioCommand::SetDevice { name, done } => {
                        // Rebuild stream on the new device
                        match find_device(&name) {
                            Some(d) => match build_stream(&d, &buf, &rate) {
                                Ok(s) => {
                                    stream = s;
                                    let _ = done.send(Ok(()));
                                }
                                Err(e) => {
                                    let _ = done.send(Err(e));
                                }
                            },
                            None => {
                                let _ = done.send(Err("Device not found".to_string()));
                            }
                        }
                    }
                    AudioCommand::Start { ready } => {
                        buf.lock().unwrap().clear();
                        if let Err(e) = stream.play() {
                            let _ = ready.send(Err(format!("Failed to start stream: {}", e)));
                            continue;
                        }
                        rec.store(true, Ordering::SeqCst);
                        let _ = ready.send(Ok(()));
                    }
                    AudioCommand::Stop { done } => {
                        rec.store(false, Ordering::SeqCst);
                        let _ = stream.pause();
                        // Let pending audio callbacks flush
                        thread::sleep(std::time::Duration::from_millis(50));
                        let _ = done.send(());
                    }
                }
            }
        });

        Self {
            buffer,
            sender: Mutex::new(tx),
            sample_rate,
        }
    }

    /// Switch to a different audio input device by name. Empty string = system default.
    pub fn set_device(&self, name: &str) -> Result<(), String> {
        let (done_tx, done_rx) = mpsc::channel();
        self.sender
            .lock()
            .unwrap()
            .send(AudioCommand::SetDevice {
                name: name.to_string(),
                done: done_tx,
            })
            .map_err(|e| format!("Failed to send set_device command: {}", e))?;
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "Timeout waiting for device switch".to_string())?
    }

    /// Starts recording. Blocks until the audio stream is confirmed running.
    pub fn start(&self) -> Result<(), String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        self.sender
            .lock()
            .unwrap()
            .send(AudioCommand::Start { ready: ready_tx })
            .map_err(|e| format!("Failed to send start command: {}", e))?;

        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "Timeout waiting for audio stream to start".to_string())?
    }

    pub fn stop(&self) -> Vec<f32> {
        let (done_tx, done_rx) = mpsc::channel();
        let _ = self.sender.lock().unwrap().send(AudioCommand::Stop { done: done_tx });
        // Wait for audio thread to pause stream and flush pending callbacks
        let _ = done_rx.recv_timeout(std::time::Duration::from_secs(2));

        let samples = self.buffer.lock().unwrap().clone();
        let rate = *self.sample_rate.lock().unwrap();

        if rate != 16000 && rate > 0 {
            resample(&samples, rate, 16000)
        } else {
            samples
        }
    }
}

/// Simple linear interpolation resampling
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < samples.len() {
            samples[idx] as f64 * (1.0 - frac) + samples[idx + 1] as f64 * frac
        } else {
            samples[idx] as f64
        };

        output.push(sample as f32);
    }

    output
}
