#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod export;
mod gemini;
mod protocol;
mod realtime;

use serde_json::json;
use tauri::{Manager, State};
use tokio::sync::{watch, Mutex};

struct Running {
    stop: watch::Sender<bool>,
    task: tauri::async_runtime::JoinHandle<()>,
}
#[derive(Default)]
struct Meeting(Mutex<Option<Running>>);

#[tauri::command]
async fn audio_devices() -> Result<capture::AudioDevices, String> {
    tauri::async_runtime::spawn_blocking(capture::devices)
        .await
        .map_err(|_| "Device lookup failed.".to_string())?
}

#[tauri::command]
async fn start_recording(
    app: tauri::AppHandle,
    state: State<'_, Meeting>,
    session_id: String,
    settings: protocol::Settings,
) -> Result<(), String> {
    settings.validate()?;
    if session_id.len() > 64 || session_id.is_empty() {
        return Err("Invalid session identifier.".into());
    }
    let mut running = state.0.lock().await;
    if running
        .as_ref()
        .is_some_and(|r| !r.task.inner().is_finished())
    {
        return Err("Recording is already running.".into());
    }
    let (stop, stopped) = watch::channel(false);
    let task = tauri::async_runtime::spawn(async move {
        let result = realtime::run(app.clone(), session_id.clone(), settings, stopped).await;
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_always_on_top(false);
        }
        match result {
            Ok(()) => realtime::emit(&app, &session_id, json!({"kind":"status", "status":"idle"})),
            Err(message) => realtime::emit(
                &app,
                &session_id,
                json!({"kind":"error", "message":message}),
            ),
        }
    });
    *running = Some(Running { stop, task });
    Ok(())
}

#[tauri::command]
async fn stop_recording(state: State<'_, Meeting>) -> Result<(), String> {
    let mut running = state.0.lock().await;
    if let Some(session) = running.take() {
        let _ = session.stop.send(true);
        session
            .task
            .await
            .map_err(|_| "Could not stop the recording task.".to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_overlay(window: tauri::WebviewWindow, opacity: f64, pinned: bool) -> Result<(), String> {
    if !opacity.is_finite() || !(0.3..=1.0).contains(&opacity) {
        return Err("Opacity must be between 30% and 100%.".into());
    }
    window
        .set_always_on_top(pinned)
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
        let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0 as _);
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
        SetLayeredWindowAttributes(
            hwnd,
            windows::Win32::Foundation::COLORREF(0),
            (opacity * 255.0).round() as u8,
            LWA_ALPHA,
        )
        .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        let mac_window = window.clone();
        window
            .run_on_main_thread(move || {
                if let Ok(handle) = mac_window.ns_window() {
                    unsafe {
                        (&*(handle as *const objc2_app_kit::NSWindow)).setAlphaValue(opacity);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Meeting::default())
        .invoke_handler(tauri::generate_handler![
            audio_devices,
            start_recording,
            stop_recording,
            set_overlay,
            export::export_transcript
        ])
        .run(tauri::generate_context!())
        .expect("Could not start Meeting Copilot");
}
