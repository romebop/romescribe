use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;

enum AudioCommand {
    Start { ready: mpsc::Sender<Result<(), String>> },
    Stop,
}

/// Thread-safe audio recorder. A dedicated audio thread is spawned at
/// construction with the device pre-initialized. Start/stop commands
/// are sent via channel for near-instant response.
pub struct AudioRecorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
    sender: Mutex<mpsc::Sender<AudioCommand>>,
    sample_rate: Arc<Mutex<u32>>,
}

unsafe impl Send for AudioRecorder {}
unsafe impl Sync for AudioRecorder {}

impl AudioRecorder {
    pub fn new() -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::new(AtomicBool::new(false));
        let sample_rate = Arc::new(Mutex::new(0u32));
        let (tx, rx) = mpsc::channel::<AudioCommand>();

        let buf = buffer.clone();
        let rec = recording.clone();
        let rate = sample_rate.clone();

        // Dedicated audio thread — device is initialized once here
        thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    eprintln!("[romescribe] No input device available");
                    return;
                }
            };

            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[romescribe] Failed to get input config: {}", e);
                    return;
                }
            };

            *rate.lock().unwrap() = config.sample_rate().0;
            let channels = config.channels() as usize;
            let sample_format = config.sample_format();
            let stream_config: cpal::StreamConfig = config.into();

            println!("[romescribe] Audio device pre-initialized (rate={}, channels={})",
                     stream_config.sample_rate.0, channels);

            // Wait for commands
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    AudioCommand::Start { ready } => {
                        buf.lock().unwrap().clear();
                        rec.store(true, Ordering::SeqCst);

                        let buf_clone = buf.clone();
                        let rec_clone = rec.clone();
                        let err_fn = |err| eprintln!("[romescribe] Audio stream error: {}", err);

                        let stream = match sample_format {
                            SampleFormat::F32 => device.build_input_stream(
                                &stream_config,
                                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                                    let mut b = buf_clone.lock().unwrap();
                                    for chunk in data.chunks(channels) {
                                        let mono: f32 =
                                            chunk.iter().sum::<f32>() / channels as f32;
                                        b.push(mono);
                                    }
                                },
                                err_fn,
                                None,
                            ),
                            SampleFormat::I16 => device.build_input_stream(
                                &stream_config,
                                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                                    let mut b = buf_clone.lock().unwrap();
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
                            ),
                            _ => {
                                let _ = ready.send(Err("Unsupported sample format".to_string()));
                                continue;
                            }
                        };

                        let stream = match stream {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = ready.send(Err(format!("Failed to build stream: {}", e)));
                                continue;
                            }
                        };

                        if let Err(e) = stream.play() {
                            let _ = ready.send(Err(format!("Failed to play stream: {}", e)));
                            continue;
                        }

                        // Signal that recording is now active
                        let _ = ready.send(Ok(()));

                        // Keep stream alive until stop
                        while rec_clone.load(Ordering::SeqCst) {
                            thread::sleep(std::time::Duration::from_millis(20));
                        }
                        drop(stream);
                    }
                    AudioCommand::Stop => {
                        rec.store(false, Ordering::SeqCst);
                    }
                }
            }
        });

        Self {
            buffer,
            recording,
            sender: Mutex::new(tx),
            sample_rate,
        }
    }

    /// Starts recording. Blocks until the audio stream is confirmed running.
    pub fn start(&self) -> Result<(), String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        self.sender
            .lock()
            .unwrap()
            .send(AudioCommand::Start { ready: ready_tx })
            .map_err(|e| format!("Failed to send start command: {}", e))?;

        // Wait for confirmation that stream is playing
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "Timeout waiting for audio stream to start".to_string())?
    }

    /// Wait until audio data is actually arriving in the buffer.
    pub fn wait_for_data(&self, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if !self.buffer.lock().unwrap().is_empty() {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn stop(&self) -> Vec<f32> {
        self.recording.store(false, Ordering::SeqCst);

        // Brief wait for stream to finish
        thread::sleep(std::time::Duration::from_millis(50));

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
