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
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use transcribe::Transcriber;

/// Insert text into the focused text field via clipboard paste (Cmd+V).
/// Peeks at the character before the cursor to decide whether to prepend a space.
/// Saves and restores the user's clipboard.
fn insert_text(text: &str) -> Result<(), String> {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    let script = format!(
        r#"
use scripting additions
tell application "System Events"
    -- Save current clipboard
    set savedClip to the clipboard

    -- Clear clipboard so empty selection = empty clipboard
    set the clipboard to ""

    -- Select char before cursor
    key code 123 using shift down
    delay 0.02

    -- Copy selection
    keystroke "c" using command down
    delay 0.05

    -- Read what we got
    set prevChar to the clipboard as text

    -- Deselect (move right to restore cursor)
    key code 124
    delay 0.02

    -- Decide whether to add space
    set needsSpace to false
    if prevChar is not "" and prevChar is not " " and prevChar is not (ASCII character 10) and prevChar is not (ASCII character 13) and prevChar is not (ASCII character 9) then
        set needsSpace to true
    end if

    -- Set clipboard to transcription text (with space if needed)
    if needsSpace then
        set the clipboard to " {escaped}"
    else
        set the clipboard to "{escaped}"
    end if

    -- Paste
    keystroke "v" using command down
    delay 0.05

    -- Restore original clipboard
    set the clipboard to savedClip
end tell
"#
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("osascript failed: {}", stderr));
    }

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
}

fn reload_transcriber(state: &AppState) -> Result<(), String> {
    let settings = state.settings.lock().unwrap().clone();
    let path = model::model_path(&settings.selected_model)?;
    if !path.exists() {
        *state.transcriber.lock().unwrap() = None;
        return Err("Model file not found".to_string());
    }
    println!(
        "[romescribe] Loading model {} (gpu={})",
        settings.selected_model, settings.use_gpu
    );
    let t = Transcriber::new(path.to_str().unwrap(), settings.use_gpu)?;
    *state.transcriber.lock().unwrap() = Some(t);
    println!("[romescribe] Model loaded successfully");
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

// --- Recording ---

#[tauri::command]
async fn toggle_recording(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    do_toggle_inner(&state, &app)
}

fn do_toggle_inner(state: &AppState, app: &AppHandle) -> Result<Option<String>, String> {
    if state.is_recording.load(Ordering::SeqCst) {
        println!("[romescribe] Stopping recording...");
        state.is_recording.store(false, Ordering::SeqCst);
        set_tray_recording(app, false);
        let _ = app.emit("recording-stopped", ());

        let audio = state.recorder.stop();
        println!("[romescribe] Got {} audio samples", audio.len());

        if audio.is_empty() {
            let _ = app.emit("error", "No audio recorded");
            return Err("No audio recorded".to_string());
        }

        let _ = app.emit("transcribing", ());

        println!("[romescribe] Starting transcription...");
        let text = {
            let transcriber_guard = state.transcriber.lock().unwrap();
            match transcriber_guard.as_ref() {
                Some(t) => t.transcribe(&audio)?,
                None => {
                    let _ = app.emit("error", "Model not loaded");
                    return Err("Model not loaded".to_string());
                }
            }
        };
        println!("[romescribe] Transcription result: {:?}", text);

        if text.is_empty() {
            let _ = app.emit("error", "No speech detected");
            return Err("No speech detected".to_string());
        }

        // Insert text directly into focused field
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Err(e) = insert_text(&text) {
            eprintln!("[romescribe] Failed to insert text: {}", e);
        }
        println!("[romescribe] Text inserted");

        let _ = app.emit("transcription-complete", &text);

        Ok(Some(text))
    } else {
        println!("[romescribe] Starting recording...");
        state.recorder.start()?;
        state.is_recording.store(true, Ordering::SeqCst);
        set_tray_recording(app, true);
        let _ = app.emit("recording-started", ());
        Ok(None)
    }
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
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    // Ignore hotkey while settings window is visible
                    if let Some(window) = app.get_webview_window("main") {
                        if window.is_visible().unwrap_or(false) {
                            return;
                        }
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

            let settings = settings::load_settings();
            println!(
                "[romescribe] Settings: model={}, gpu={}, hotkey={}",
                settings.selected_model, settings.use_gpu, settings.hotkey
            );

            // Load model if available
            let transcriber = if model::is_model_downloaded(&settings.selected_model) {
                let path = model::model_path(&settings.selected_model)?;
                match Transcriber::new(path.to_str().unwrap(), settings.use_gpu) {
                    Ok(t) => {
                        println!("[romescribe] Whisper model loaded successfully");
                        Some(t)
                    }
                    Err(e) => {
                        eprintln!("[romescribe] Failed to load model: {}", e);
                        None
                    }
                }
            } else {
                eprintln!(
                    "[romescribe] Model '{}' not found. Use settings to download it.",
                    settings.selected_model
                );
                None
            };

            // Register hotkey from settings
            let shortcut = parse_hotkey(&settings.hotkey).unwrap_or_else(|_| {
                eprintln!("[romescribe] Invalid hotkey '{}', falling back to Cmd+Shift+R", settings.hotkey);
                Shortcut::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyR)
            });

            app.global_shortcut().register(shortcut.clone()).map_err(|e| {
                eprintln!("[romescribe] Failed to register global shortcut: {}", e);
                e
            })?;

            // System tray: Settings + Quit only
            let settings_item = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &sep, &quit_item])?;

            app.manage(AppState {
                recorder: AudioRecorder::new(),
                transcriber: Mutex::new(transcriber),
                is_recording: AtomicBool::new(false),
                settings: Mutex::new(settings),
                downloading: Mutex::new(None),
                cancel_download: AtomicBool::new(false),
                current_shortcut: Mutex::new(Some(shortcut)),
                record_start_time: Mutex::new(None),
            });

            let _tray = TrayIconBuilder::with_id("main")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/icon-idle.png")).unwrap())
                .menu(&menu)
                .tooltip("romescribe")
                .on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "settings" => {
                            let _ = app.emit("navigate", "settings");
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Don't close — just hide the window
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
            download_model,
            cancel_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
