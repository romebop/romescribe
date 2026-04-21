mod audio;
mod model;
mod settings;
mod transcribe;

use audio::AudioRecorder;
use settings::Settings;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State,
};
use core_graphics::event::{CGEvent, CGEventFlags, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::sync::mpsc as std_mpsc;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use transcribe::Transcriber;

const KVK_ANSI_V: CGKeyCode = 0x09;

/// Show the main window and navigate to a view.
fn show_window(app: &AppHandle, view: &str) {
    let _ = app.emit("navigate", view);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn is_accessibility_granted() -> bool {
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

fn check_accessibility() -> bool {
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
        static kCFTypeDictionaryKeyCallBacks: std::ffi::c_void;
        static kCFTypeDictionaryValueCallBacks: std::ffi::c_void;
        static kCFBooleanTrue: *const std::ffi::c_void;
        fn CFDictionaryCreate(
            alloc: *const std::ffi::c_void,
            keys: *const *const std::ffi::c_void,
            vals: *const *const std::ffi::c_void,
            n: i64,
            kc: *const std::ffi::c_void,
            vc: *const std::ffi::c_void,
        ) -> *const std::ffi::c_void;
    }
    unsafe {
        let key = kAXTrustedCheckOptionPrompt;
        let val = kCFBooleanTrue;
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            &key,
            &val,
            1,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const std::ffi::c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const std::ffi::c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(dict);
        if !trusted {
            println!("[romescribe] Accessibility not enabled — prompt shown");
        }
        trusted
    }
}

/// Simulate Cmd+V via CGEvent (direct C API, no subprocess).
fn simulate_paste() -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "Failed to create event source")?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KVK_ANSI_V, true)
        .map_err(|_| "Failed to create key down event")?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);

    let key_up = CGEvent::new_keyboard_event(source, KVK_ANSI_V, false)
        .map_err(|_| "Failed to create key up event")?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    key_down.post(core_graphics::event::CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(10));
    key_up.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

/// Insert text into the focused text field via clipboard paste.
/// Uses Tauri clipboard API + CGEvent Cmd+V. Saves and restores clipboard.
fn insert_text(app: &AppHandle, text: &str) -> Result<(), String> {
    // Save current clipboard
    let saved = app.clipboard().read_text().unwrap_or_default();

    // Set transcription text
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("Clipboard write error: {}", e))?;

    // Cmd+V via CGEvent
    std::thread::sleep(std::time::Duration::from_millis(10));
    simulate_paste()?;

    // Restore clipboard after paste has been processed
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = app.clipboard().write_text(&saved);

    Ok(())
}

// --- Hotkey Parsing ---

fn parse_hotkey(hotkey_str: &str) -> Result<Shortcut, String> {
    let parts: Vec<&str> = hotkey_str.split('+').map(|s| s.trim()).collect();
    let mut modifiers = Modifiers::empty();
    let mut key_code: Option<Code> = None;

    for part in &parts {
        match part.to_lowercase().as_str() {
            "cmd" | "meta" | "command" | "super" => modifiers |= Modifiers::META,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" | "option" | "opt" => modifiers |= Modifiers::ALT,
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            key => {
                key_code = Some(parse_key_code(key)?);
            }
        }
    }

    let code = key_code.ok_or("No key specified in hotkey")?;
    let mods = if modifiers.is_empty() { None } else { Some(modifiers) };
    Ok(Shortcut::new(mods, code))
}

fn parse_key_code(key: &str) -> Result<Code, String> {
    match key.to_uppercase().as_str() {
        "A" => Ok(Code::KeyA), "B" => Ok(Code::KeyB), "C" => Ok(Code::KeyC),
        "D" => Ok(Code::KeyD), "E" => Ok(Code::KeyE), "F" => Ok(Code::KeyF),
        "G" => Ok(Code::KeyG), "H" => Ok(Code::KeyH), "I" => Ok(Code::KeyI),
        "J" => Ok(Code::KeyJ), "K" => Ok(Code::KeyK), "L" => Ok(Code::KeyL),
        "M" => Ok(Code::KeyM), "N" => Ok(Code::KeyN), "O" => Ok(Code::KeyO),
        "P" => Ok(Code::KeyP), "Q" => Ok(Code::KeyQ), "R" => Ok(Code::KeyR),
        "S" => Ok(Code::KeyS), "T" => Ok(Code::KeyT), "U" => Ok(Code::KeyU),
        "V" => Ok(Code::KeyV), "W" => Ok(Code::KeyW), "X" => Ok(Code::KeyX),
        "Y" => Ok(Code::KeyY), "Z" => Ok(Code::KeyZ),
        "0" => Ok(Code::Digit0), "1" => Ok(Code::Digit1), "2" => Ok(Code::Digit2),
        "3" => Ok(Code::Digit3), "4" => Ok(Code::Digit4), "5" => Ok(Code::Digit5),
        "6" => Ok(Code::Digit6), "7" => Ok(Code::Digit7), "8" => Ok(Code::Digit8),
        "9" => Ok(Code::Digit9),
        "SPACE" => Ok(Code::Space),
        "ENTER" | "RETURN" => Ok(Code::Enter),
        "ESCAPE" | "ESC" => Ok(Code::Escape),
        "TAB" => Ok(Code::Tab),
        "F1" => Ok(Code::F1), "F2" => Ok(Code::F2), "F3" => Ok(Code::F3),
        "F4" => Ok(Code::F4), "F5" => Ok(Code::F5), "F6" => Ok(Code::F6),
        "F7" => Ok(Code::F7), "F8" => Ok(Code::F8), "F9" => Ok(Code::F9),
        "F10" => Ok(Code::F10), "F11" => Ok(Code::F11), "F12" => Ok(Code::F12),
        _ => Err(format!("Unknown key: {}", key)),
    }
}

// --- Tray Icon ---

fn set_tray_recording(app: &AppHandle, recording: bool) {
    if let Some(tray) = app.tray_by_id("main") {
        let icon_bytes: &[u8] = if recording {
            include_bytes!("../icons/icon-recording.png")
        } else {
            include_bytes!("../icons/icon-idle.png")
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(icon));
        }
        let _ = tray.set_tooltip(Some(if recording { "romescribe - Recording..." } else { "romescribe" }));
    }
}

// --- App State ---

struct AppState {
    recorder: AudioRecorder,
    transcriber: Mutex<Option<Transcriber>>,
    is_recording: AtomicBool,
    settings: Mutex<Settings>,
    downloading: Mutex<Option<String>>,
    cancel_download: AtomicBool,
    current_shortcut: Mutex<Option<Shortcut>>,
    record_start_time: Mutex<Option<std::time::Instant>>,
    recording_began: Mutex<Option<std::time::Instant>>,
    toggle_lock: Mutex<()>,
    logs: Mutex<Vec<String>>,
}

fn add_log(state: &AppState, msg: &str) {
    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    let entry = format!("[{}] {}", timestamp, msg);
    println!("{}", entry);
    state.logs.lock().unwrap().push(entry);
}

fn reload_transcriber(state: &AppState) -> Result<(), String> {
    let settings = state.settings.lock().unwrap().clone();
    let path = model::model_path(&settings.selected_model)?;
    if !path.exists() {
        *state.transcriber.lock().unwrap() = None;
        return Err(format!("Model '{}' not found", settings.selected_model));
    }
    add_log(state, &format!("Loading model {} (gpu={})", settings.selected_model, settings.use_gpu));
    let t = Transcriber::new(path.to_str().unwrap(), settings.use_gpu)?;
    *state.transcriber.lock().unwrap() = Some(t);
    add_log(state, "Model loaded");
    Ok(())
}

// --- Tauri Commands ---

#[tauri::command]
fn get_model_status(state: State<'_, AppState>) -> bool {
    state.transcriber.lock().unwrap().is_some()
}

#[tauri::command]
fn get_recording_status(state: State<'_, AppState>) -> bool {
    state.is_recording.load(Ordering::SeqCst)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[derive(serde::Serialize)]
struct ModelEntry {
    id: String,
    name: String,
    description: String,
    size_bytes: u64,
    downloaded: bool,
    active: bool,
}

#[tauri::command]
fn get_models(state: State<'_, AppState>) -> Vec<ModelEntry> {
    let selected = state.settings.lock().unwrap().selected_model.clone();
    model::MODELS
        .iter()
        .map(|m| ModelEntry {
            id: m.id.to_string(),
            name: m.name.to_string(),
            description: m.description.to_string(),
            size_bytes: m.size_bytes,
            downloaded: model::is_model_downloaded(m.id),
            active: m.id == selected,
        })
        .collect()
}

#[tauri::command]
fn select_model(model_id: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if state.is_recording.load(Ordering::SeqCst) {
        return Err("Cannot change model while recording".to_string());
    }

    model::get_model_info(&model_id).ok_or(format!("Unknown model: {}", model_id))?;

    if !model::is_model_downloaded(&model_id) {
        return Err("Model not downloaded yet".to_string());
    }

    {
        let mut settings = state.settings.lock().unwrap();
        settings.selected_model = model_id.clone();
        settings::save_settings(&settings)?;
    }

    let _ = app.emit("model-loading", ());
    reload_transcriber(state.inner())?;
    let _ = app.emit("model-loaded", ());

    Ok(())
}

#[tauri::command]
fn set_use_gpu(use_gpu: bool, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    if state.is_recording.load(Ordering::SeqCst) {
        return Err("Cannot change GPU setting while recording".to_string());
    }

    {
        let mut settings = state.settings.lock().unwrap();
        settings.use_gpu = use_gpu;
        settings::save_settings(&settings)?;
    }

    if state.transcriber.lock().unwrap().is_some() {
        let _ = app.emit("model-loading", ());
        reload_transcriber(state.inner())?;
        let _ = app.emit("model-loaded", ());
    }

    Ok(())
}

#[tauri::command]
fn set_hotkey(hotkey: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let new_shortcut = parse_hotkey(&hotkey)?;

    {
        let current = state.current_shortcut.lock().unwrap();
        if let Some(old) = current.as_ref() {
            let _ = app.global_shortcut().unregister(old.clone());
        }
    }

    app.global_shortcut()
        .register(new_shortcut.clone())
        .map_err(|e| format!("Failed to register hotkey: {}", e))?;

    *state.current_shortcut.lock().unwrap() = Some(new_shortcut);
    {
        let mut settings = state.settings.lock().unwrap();
        settings.hotkey = hotkey;
        settings::save_settings(&settings)?;
    }

    Ok(())
}

#[tauri::command]
fn set_copy_to_clipboard(copy_to_clipboard: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    settings.copy_to_clipboard = copy_to_clipboard;
    settings::save_settings(&settings)
}

#[tauri::command]
fn get_audio_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn set_audio_device(device: String, state: State<'_, AppState>) -> Result<(), String> {
    if state.is_recording.load(Ordering::SeqCst) {
        return Err("Cannot change audio device while recording".to_string());
    }
    state.recorder.set_device(&device)?;
    let mut settings = state.settings.lock().unwrap();
    settings.audio_device = device;
    settings::save_settings(&settings)
}

#[derive(Clone, serde::Serialize)]
struct DownloadProgress {
    model_id: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}

#[tauri::command]
fn download_model(model_id: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    {
        let downloading = state.downloading.lock().unwrap();
        if downloading.is_some() {
            return Err("A download is already in progress".to_string());
        }
    }

    let info = model::get_model_info(&model_id)
        .ok_or(format!("Unknown model: {}", model_id))?;

    if model::is_model_downloaded(&model_id) {
        return Err("Model already downloaded".to_string());
    }

    *state.downloading.lock().unwrap() = Some(model_id.clone());
    state.cancel_download.store(false, Ordering::SeqCst);

    let url = info.url.to_string();
    let filename = info.filename.to_string();
    let total_size = info.size_bytes;
    let model_id_clone = model_id.clone();

    let app_clone = app.clone();

    std::thread::spawn(move || {
        let state_ref = app_clone.state::<AppState>();
        let model_id = model_id_clone;

        let result = (|| -> Result<(), String> {
            let models_dir = model::models_dir()?;
            let part_path = models_dir.join(format!("{}.part", filename));
            let final_path = models_dir.join(&filename);

            let client = reqwest::blocking::Client::new();
            let mut response = client
                .get(&url)
                .send()
                .map_err(|e| format!("Download failed: {}", e))?;

            if !response.status().is_success() {
                return Err(format!("Download failed with status: {}", response.status()));
            }

            let mut file = std::fs::File::create(&part_path)
                .map_err(|e| format!("Failed to create file: {}", e))?;

            let mut downloaded: u64 = 0;
            let mut last_emit = std::time::Instant::now();
            let mut buffer = [0u8; 65536];

            loop {
                if state_ref.cancel_download.load(Ordering::SeqCst) {
                    let _ = std::fs::remove_file(&part_path);
                    return Err("Download cancelled".to_string());
                }

                let bytes_read = response
                    .read(&mut buffer)
                    .map_err(|e| format!("Read error: {}", e))?;

                if bytes_read == 0 {
                    break;
                }

                std::io::Write::write_all(&mut file, &buffer[..bytes_read])
                    .map_err(|e| format!("Write error: {}", e))?;

                downloaded += bytes_read as u64;

                if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                    let _ = app_clone.emit(
                        "download-progress",
                        DownloadProgress {
                            model_id: model_id.clone(),
                            downloaded_bytes: downloaded,
                            total_bytes: total_size,
                        },
                    );
                    last_emit = std::time::Instant::now();
                }
            }

            std::fs::rename(&part_path, &final_path)
                .map_err(|e| format!("Failed to finalize download: {}", e))?;

            let _ = app_clone.emit(
                "download-progress",
                DownloadProgress {
                    model_id: model_id.clone(),
                    downloaded_bytes: total_size,
                    total_bytes: total_size,
                },
            );

            Ok(())
        })();

        *state_ref.downloading.lock().unwrap() = None;

        match result {
            Ok(()) => {
                let _ = app_clone.emit("download-complete", &model_id);
                println!("[romescribe] Download complete: {}", model_id);
            }
            Err(e) => {
                let _ = app_clone.emit("download-error", &e);
                eprintln!("[romescribe] Download error: {}", e);
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn cancel_download(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel_download.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn get_logs(state: State<'_, AppState>) -> Vec<String> {
    state.logs.lock().unwrap().clone()
}

// --- Recording ---

#[tauri::command]
async fn toggle_recording(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    do_toggle_inner(&state, &app)
}

fn play_sound(name: &str) {
    let path = format!("/System/Library/Sounds/{}.aiff", name);
    std::thread::spawn(move || {
        let _ = std::process::Command::new("afplay").arg(&path).output();
    });
}

fn do_toggle_inner(state: &AppState, app: &AppHandle) -> Result<Option<String>, String> {
    let _guard = state.toggle_lock.lock().unwrap();

    if state.is_recording.load(Ordering::SeqCst) {
        play_sound("Pop");
        add_log(state, "Stopping recording...");

        // Ensure minimum recording duration for reliable transcription
        if let Some(began) = *state.recording_began.lock().unwrap() {
            let min_duration = std::time::Duration::from_millis(750);
            let elapsed = began.elapsed();
            if elapsed < min_duration {
                std::thread::sleep(min_duration - elapsed);
            }
        }
        state.is_recording.store(false, Ordering::SeqCst);
        set_tray_recording(app, false);
        let _ = app.emit("recording-stopped", ());

        // Unregister Escape so it doesn't interfere with other apps
        let escape = Shortcut::new(None, Code::Escape);
        match app.global_shortcut().unregister(escape) {
            Ok(_) => println!("[romescribe] Escape shortcut unregistered"),
            Err(e) => eprintln!("[romescribe] Failed to unregister Escape shortcut: {}", e),
        }

        let audio = state.recorder.stop();
        add_log(state, &format!("Got {} audio samples", audio.len()));

        if audio.is_empty() {
            add_log(state, "Error: No audio recorded");
            let _ = app.emit("error", "No audio recorded");
            return Err("No audio recorded".to_string());
        }

        // Check audio energy — Whisper hallucinates on near-silence
        let rms = (audio.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / audio.len() as f64).sqrt();
        add_log(state, &format!("Audio RMS energy: {:.6}", rms));
        if rms < 0.001 {
            add_log(state, "Audio too quiet — skipping transcription");
            let _ = app.emit("error", "No speech detected (audio too quiet)");
            return Err("No speech detected".to_string());
        }

        // Pad short audio with silence — Whisper struggles with short clips
        let min_samples = 24000; // 1.5 seconds at 16kHz
        let audio = if audio.len() < min_samples {
            let mut padded = audio;
            padded.resize(min_samples, 0.0);
            padded
        } else {
            audio
        };

        let _ = app.emit("transcribing", ());

        add_log(state, "Transcribing...");
        let text = {
            let transcriber_guard = state.transcriber.lock().unwrap();
            match transcriber_guard.as_ref() {
                Some(t) => {
                    let model_id = state.settings.lock().unwrap().selected_model.clone();
                    let english_only = model_id.ends_with(".en");
                    t.transcribe(&audio, english_only)?
                }
                None => {
                    add_log(state, "Error: Model not loaded");
                    let _ = app.emit("error", "Model not loaded");
                    return Err("Model not loaded".to_string());
                }
            }
        };
        add_log(state, &format!("Transcription: \"{}\"", text));

        if text.is_empty() {
            add_log(state, "No speech detected");
            let _ = app.emit("error", "No speech detected");
            return Err("No speech detected".to_string());
        }

        // Insert text directly into focused field
        let copy_to_clipboard = state.settings.lock().unwrap().copy_to_clipboard;
        std::thread::sleep(std::time::Duration::from_millis(50));
        if copy_to_clipboard {
            // Write to clipboard and paste, but don't restore old clipboard contents
            let saved = app.clipboard().read_text().unwrap_or_default();
            app.clipboard().write_text(&text).ok();
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Err(e) = simulate_paste() {
                add_log(state, &format!("Insert failed: {}", e));
                // Restore on failure
                let _ = app.clipboard().write_text(&saved);
            } else {
                add_log(state, "Text inserted (clipboard retained)");
            }
        } else if let Err(e) = insert_text(app, &text) {
            add_log(state, &format!("Insert failed: {}", e));
        } else {
            add_log(state, "Text inserted");
        }

        let _ = app.emit("transcription-complete", &text);

        Ok(Some(text))
    } else {
        state.recorder.start()?;
        *state.recording_began.lock().unwrap() = Some(std::time::Instant::now());
        state.is_recording.store(true, Ordering::SeqCst);
        set_tray_recording(app, true);
        let _ = app.emit("recording-started", ());
        add_log(state, "Recording started");

        // Register Escape globally only while recording
        let escape = Shortcut::new(None, Code::Escape);
        match app.global_shortcut().register(escape) {
            Ok(_) => println!("[romescribe] Escape shortcut registered"),
            Err(e) => eprintln!("[romescribe] Failed to register Escape shortcut: {}", e),
        }

        // Stream live audio peak to the overlay ~30Hz until recording stops.
        let app_for_levels = app.clone();
        std::thread::spawn(move || {
            let state = app_for_levels.state::<AppState>();
            while state.is_recording.load(Ordering::SeqCst) {
                let spectrum = state.recorder.take_spectrum();
                let _ = app_for_levels.emit("audio-level", spectrum);
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
        });

        Ok(None)
    }
}

fn do_cancel(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.is_recording.load(Ordering::SeqCst) {
        return;
    }
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let state = app_clone.state::<AppState>();
        let _guard = state.toggle_lock.lock().unwrap();
        if !state.is_recording.load(Ordering::SeqCst) {
            return;
        }
        add_log(state.inner(), "Recording cancelled");
        state.is_recording.store(false, Ordering::SeqCst);
        set_tray_recording(&app_clone, false);
        let _ = app_clone.emit("recording-stopped", ());
        let _ = app_clone.emit("recording-cancelled", ());

        // Unregister Escape so it doesn't interfere with other apps
        let escape = Shortcut::new(None, Code::Escape);
        match app_clone.global_shortcut().unregister(escape) {
            Ok(_) => println!("[romescribe] Escape shortcut unregistered (cancel)"),
            Err(e) => eprintln!("[romescribe] Failed to unregister Escape shortcut: {}", e),
        }

        // Discard the audio
        state.recorder.stop();
    });
}

fn do_toggle(app: &AppHandle) {
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let state = app_clone.state::<AppState>();
        let result = do_toggle_inner(state.inner(), &app_clone);
        if let Err(e) = result {
            eprintln!("[romescribe] Toggle error: {}", e);
            let _ = app_clone.emit("error", e);
        }
    });
}

// --- App Setup ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
// --- macOS sleep/wake detection via IOKit ---

struct PowerContext {
    wake_sender: std_mpsc::Sender<()>,
    root_port: u32,
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut std::ffi::c_void,
        port_ref: *mut *mut std::ffi::c_void,
        callback: extern "C" fn(
            refcon: *mut std::ffi::c_void,
            service: u32,
            message_type: u32,
            message_argument: *mut std::ffi::c_void,
        ),
        notifier: *mut u32,
    ) -> u32;
    fn IONotificationPortGetRunLoopSource(notify: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn IOAllowPowerChange(kernel_port: u32, notification_id: isize);
    fn CFRunLoopGetCurrent() -> *mut std::ffi::c_void;
    fn CFRunLoopAddSource(
        rl: *mut std::ffi::c_void,
        source: *mut std::ffi::c_void,
        mode: *const std::ffi::c_void,
    );
    fn CFRunLoopRun();
}

extern "C" {
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
}

const KIO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;
const KIO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0xe000_0240;
const KIO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;

extern "C" fn power_callback(
    refcon: *mut std::ffi::c_void,
    _service: u32,
    message_type: u32,
    message_argument: *mut std::ffi::c_void,
) {
    let ctx = unsafe { &*(refcon as *const PowerContext) };
    match message_type {
        KIO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
            let _ = ctx.wake_sender.send(());
        }
        KIO_MESSAGE_CAN_SYSTEM_SLEEP | KIO_MESSAGE_SYSTEM_WILL_SLEEP => {
            unsafe { IOAllowPowerChange(ctx.root_port, message_argument as isize) };
        }
        _ => {}
    }
}

// --- macOS display wake detection via NSWorkspace ---

static DISPLAY_WAKE_SENDER: std::sync::OnceLock<std_mpsc::Sender<()>> = std::sync::OnceLock::new();

type ObjcId = *mut std::ffi::c_void;
type ObjcSel = *const std::ffi::c_void;
type ObjcClass = *mut std::ffi::c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const std::os::raw::c_char) -> ObjcClass;
    fn objc_allocateClassPair(
        superclass: ObjcClass,
        name: *const std::os::raw::c_char,
        extra_bytes: usize,
    ) -> ObjcClass;
    fn objc_registerClassPair(cls: ObjcClass);
    fn class_addMethod(
        cls: ObjcClass,
        sel: ObjcSel,
        imp: *const std::ffi::c_void,
        types: *const std::os::raw::c_char,
    ) -> bool;
    fn sel_registerName(name: *const std::os::raw::c_char) -> ObjcSel;
    fn objc_msgSend();
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    static NSWorkspaceScreensDidWakeNotification: ObjcId;
}

extern "C" fn handle_display_wake(_self: ObjcId, _cmd: ObjcSel, _notif: ObjcId) {
    println!("[romescribe] Display wake notification received");
    if let Some(tx) = DISPLAY_WAKE_SENDER.get() {
        let _ = tx.send(());
    }
}

unsafe fn register_display_wake_observer() {
    unsafe fn msg_send_0(receiver: ObjcId, sel: ObjcSel) -> ObjcId {
        let f: extern "C" fn(ObjcId, ObjcSel) -> ObjcId =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, sel)
    }
    unsafe fn msg_send_add_observer(
        center: ObjcId,
        sel: ObjcSel,
        observer: ObjcId,
        selector: ObjcSel,
        name: ObjcId,
        object: ObjcId,
    ) {
        let f: extern "C" fn(ObjcId, ObjcSel, ObjcId, ObjcSel, ObjcId, ObjcId) =
            std::mem::transmute(objc_msgSend as *const ());
        f(center, sel, observer, selector, name, object);
    }

    let nsobject = objc_getClass(c"NSObject".as_ptr());
    if nsobject.is_null() {
        eprintln!("[romescribe] Failed to find NSObject class");
        return;
    }
    let cls = objc_allocateClassPair(nsobject, c"RomescribeDisplayObserver".as_ptr(), 0);
    if cls.is_null() {
        eprintln!("[romescribe] Failed to allocate display observer class");
        return;
    }
    let sel_handle = sel_registerName(c"handleDisplayWake:".as_ptr());
    let ok = class_addMethod(
        cls,
        sel_handle,
        handle_display_wake as *const std::ffi::c_void,
        c"v@:@".as_ptr(),
    );
    if !ok {
        eprintln!("[romescribe] Failed to add method to display observer class");
        return;
    }
    objc_registerClassPair(cls);

    let alloc_sel = sel_registerName(c"alloc".as_ptr());
    let init_sel = sel_registerName(c"init".as_ptr());
    let instance = msg_send_0(msg_send_0(cls, alloc_sel), init_sel);
    if instance.is_null() {
        eprintln!("[romescribe] Failed to create display observer instance");
        return;
    }

    let nsworkspace = objc_getClass(c"NSWorkspace".as_ptr());
    let shared_sel = sel_registerName(c"sharedWorkspace".as_ptr());
    let workspace = msg_send_0(nsworkspace, shared_sel);

    let center_sel = sel_registerName(c"notificationCenter".as_ptr());
    let center = msg_send_0(workspace, center_sel);

    let add_sel = sel_registerName(c"addObserver:selector:name:object:".as_ptr());
    msg_send_add_observer(
        center,
        add_sel,
        instance,
        sel_handle,
        NSWorkspaceScreensDidWakeNotification,
        std::ptr::null_mut(),
    );

    println!("[romescribe] Display wake observer registered");
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    let escape = Shortcut::new(None, Code::Escape);
                    if shortcut == &escape {
                        if event.state() == ShortcutState::Pressed {
                            do_cancel(app);
                        }
                        return;
                    }

                    let state = app.state::<AppState>();
                    match event.state() {
                        ShortcutState::Pressed => {
                            if state.is_recording.load(Ordering::SeqCst) {
                                // Already recording — this is a toggle-off (tap mode)
                                *state.record_start_time.lock().unwrap() = None;
                                do_toggle(app);
                            } else {
                                // Start recording, note the time for hold detection
                                *state.record_start_time.lock().unwrap() = Some(std::time::Instant::now());
                                play_sound("Tink");
                                do_toggle(app);
                            }
                        }
                        ShortcutState::Released => {
                            // Only stop on release if we've been holding for >300ms (hold mode)
                            let start = state.record_start_time.lock().unwrap().take();
                            if let Some(t) = start {
                                if t.elapsed() > std::time::Duration::from_millis(300)
                                    && state.is_recording.load(Ordering::SeqCst)
                                {
                                    do_toggle(app);
                                }
                            }
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            // Hide from Dock — run as menu bar only app
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Overlay is a floating indicator — must never steal clicks.
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
            }

            let accessibility_granted = check_accessibility();

            let settings = settings::load_settings();
            println!(
                "[romescribe] Settings: model={}, gpu={}, hotkey={}",
                settings.selected_model, settings.use_gpu, settings.hotkey
            );

            // Load model if available, or auto-download small.en on first launch
            let transcriber = if model::is_model_downloaded(&settings.selected_model) {
                let path = model::model_path(&settings.selected_model)?;
                match Transcriber::new(path.to_str().unwrap(), settings.use_gpu) {
                    Ok(t) => {
                        println!("[romescribe] Model '{}' loaded", settings.selected_model);
                        Some(t)
                    }
                    Err(e) => {
                        eprintln!("[romescribe] Failed to load model: {}", e);
                        None
                    }
                }
            } else {
                // Auto-download default model on first launch
                let default_model = "small.en";
                println!("[romescribe] No model found. Auto-downloading {}...", default_model);
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let info = match model::get_model_info(default_model) {
                        Some(i) => i,
                        None => return,
                    };

                    let models_dir = match model::models_dir() {
                        Ok(d) => d,
                        Err(_) => return,
                    };

                    let part_path = models_dir.join(format!("{}.part", info.filename));
                    let final_path = models_dir.join(info.filename);

                    let client = match reqwest::blocking::Client::new()
                        .get(info.url)
                        .send() {
                        Ok(r) if r.status().is_success() => r,
                        _ => {
                            eprintln!("[romescribe] Failed to download default model");
                            return;
                        }
                    };

                    let mut response = client;
                    let mut file = match std::fs::File::create(&part_path) {
                        Ok(f) => f,
                        Err(_) => return,
                    };

                    let mut downloaded: u64 = 0;
                    let mut last_emit = std::time::Instant::now();
                    let mut buffer = [0u8; 65536];
                    let total_size = info.size_bytes;

                    loop {
                        let bytes_read = match response.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };

                        if std::io::Write::write_all(&mut file, &buffer[..bytes_read]).is_err() {
                            break;
                        }

                        downloaded += bytes_read as u64;

                        if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                            let _ = app_handle.emit(
                                "download-progress",
                                DownloadProgress {
                                    model_id: default_model.to_string(),
                                    downloaded_bytes: downloaded,
                                    total_bytes: total_size,
                                },
                            );
                            last_emit = std::time::Instant::now();
                        }
                    }

                    if std::fs::rename(&part_path, &final_path).is_err() {
                        return;
                    }

                    println!("[romescribe] Default model downloaded. Loading...");

                    // Update settings and load the model
                    let state = app_handle.state::<AppState>();
                    {
                        let mut s = state.settings.lock().unwrap();
                        s.selected_model = default_model.to_string();
                        let _ = settings::save_settings(&s);
                    }
                    let use_gpu = state.settings.lock().unwrap().use_gpu;
                    if let Ok(path) = model::model_path(default_model) {
                        if let Ok(t) = Transcriber::new(path.to_str().unwrap(), use_gpu) {
                            *state.transcriber.lock().unwrap() = Some(t);
                            add_log(&state, "Default model (small.en) ready");
                            let _ = app_handle.emit("model-loaded", ());
                            println!("[romescribe] Model '{}' loaded and ready", default_model);
                        }
                    }
                });
                None
            };

            // Register hotkey from settings
            let shortcut = parse_hotkey(&settings.hotkey).unwrap_or_else(|_| {
                eprintln!("[romescribe] Invalid hotkey '{}', falling back to Cmd+Shift+R", settings.hotkey);
                Shortcut::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyR)
            });

            if accessibility_granted {
                app.global_shortcut().register(shortcut.clone()).map_err(|e| {
                    eprintln!("[romescribe] Failed to register global shortcut: {}", e);
                    e
                })?;

                // Escape shortcut is registered dynamically only while recording
            } else {
                eprintln!("[romescribe] Skipping shortcut registration — waiting for accessibility...");
                let app_handle = app.handle().clone();
                let shortcut_clone = shortcut.clone();
                std::thread::spawn(move || {
                    // Poll until accessibility is granted
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if is_accessibility_granted() {
                            println!("[romescribe] Accessibility granted — registering shortcuts");
                            if let Err(e) = app_handle.global_shortcut().register(shortcut_clone) {
                                eprintln!("[romescribe] Failed to register hotkey: {}", e);
                            }
                            // Escape shortcut is registered dynamically only while recording
                            break;
                        }
                    }
                });
            }

            // System tray
            let settings_item = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let logs_item = MenuItem::with_id(app, "logs", "Session Logs...", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &logs_item, &sep, &quit_item])?;

            app.manage(AppState {
                recorder: AudioRecorder::new(&settings.audio_device),
                transcriber: Mutex::new(transcriber),
                is_recording: AtomicBool::new(false),
                settings: Mutex::new(settings),
                downloading: Mutex::new(None),
                cancel_download: AtomicBool::new(false),
                current_shortcut: Mutex::new(Some(shortcut)),
                record_start_time: Mutex::new(None),
                recording_began: Mutex::new(None),
                toggle_lock: Mutex::new(()),
                logs: Mutex::new(Vec::new()),
            });

            let _tray = TrayIconBuilder::with_id("main")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/icon-idle.png")).unwrap())
                .menu(&menu)
                .tooltip("romescribe")
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "settings" => {
                            show_window(app, "settings");
                        }
                        "logs" => {
                            show_window(app, "logs");
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            // Listen for macOS sleep/wake to rebuild audio stream
            let (wake_tx, wake_rx) = std_mpsc::channel::<()>();

            // Register NSWorkspace display wake observer (fires on screen-only sleep/wake)
            let _ = DISPLAY_WAKE_SENDER.set(wake_tx.clone());
            unsafe { register_display_wake_observer(); }

            std::thread::spawn(move || {
                unsafe {
                    let ctx = Box::into_raw(Box::new(PowerContext {
                        wake_sender: wake_tx,
                        root_port: 0,
                    }));
                    let mut port: *mut std::ffi::c_void = std::ptr::null_mut();
                    let mut notifier: u32 = 0;
                    let root_port = IORegisterForSystemPower(
                        ctx as *mut std::ffi::c_void,
                        &mut port,
                        power_callback,
                        &mut notifier,
                    );
                    if root_port == 0 {
                        eprintln!("[romescribe] Failed to register for power notifications");
                        drop(Box::from_raw(ctx));
                        return;
                    }
                    (*ctx).root_port = root_port;
                    let source = IONotificationPortGetRunLoopSource(port);
                    let rl = CFRunLoopGetCurrent();
                    CFRunLoopAddSource(rl, source, kCFRunLoopDefaultMode);
                    CFRunLoopRun();
                }
            });

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                while wake_rx.recv().is_ok() {
                    println!("[romescribe] System wake detected — rebuilding audio stream in 1s");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let state = app_handle.state::<AppState>();
                    match state.recorder.rebuild() {
                        Ok(()) => println!("[romescribe] Audio stream rebuilt after wake"),
                        Err(e) => eprintln!("[romescribe] Failed to rebuild audio after wake: {}", e),
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            toggle_recording,
            get_model_status,
            get_recording_status,
            get_settings,
            get_models,
            select_model,
            set_use_gpu,
            set_hotkey,
            set_copy_to_clipboard,
            get_audio_devices,
            set_audio_device,
            download_model,
            cancel_download,
            get_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
