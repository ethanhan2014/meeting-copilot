use std::{fs::OpenOptions, io::Write, path::Path};

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or("Invalid export filename")?
        .to_string_lossy();
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "Invalid system time")?
        .as_nanos();
    let temporary = path.with_file_name(format!(".{name}.{}.{suffix}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })()
    .map_err(|e: std::io::Error| {
        format!("Export could not be saved: {e}. The transcript remains in the app.")
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[tauri::command]
pub async fn export_transcript(
    window: tauri::WebviewWindow,
    text: String,
    filename: String,
) -> Result<Option<String>, String> {
    if filename.is_empty()
        || filename.len() > 120
        || filename.contains(['/', '\\', '\0'])
        || !filename.ends_with(".md")
    {
        return Err("Invalid export filename".into());
    }
    #[cfg(windows)]
    {
        let owner = window.hwnd().map_err(|e| e.to_string())?.0 as usize;
        tauri::async_runtime::spawn_blocking(move || save_dialog(owner, &text, &filename))
            .await
            .map_err(|_| "Export dialog failed".to_string())?
    }
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let _ = sender.send(macos_save_path(&filename));
            })
            .map_err(|e| e.to_string())?;
        let Some(path) = receiver
            .await
            .map_err(|_| "Export dialog failed".to_string())??
        else {
            return Ok(None);
        };
        tauri::async_runtime::spawn_blocking(move || {
            write_atomic(&path, &text)?;
            Ok(Some(path.to_string_lossy().into_owned()))
        })
        .await
        .map_err(|_| "Export could not be saved".to_string())?
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (window, text, filename);
        Err("Run the Windows desktop app to export a session.".into())
    }
}

#[cfg(target_os = "macos")]
fn macos_save_path(filename: &str) -> Result<Option<std::path::PathBuf>, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSSavePanel};
    use objc2_foundation::{NSArray, NSString};
    let main_thread = MainThreadMarker::new().ok_or("Export dialog requires the main thread")?;
    let panel = NSSavePanel::savePanel(main_thread);
    panel.setNameFieldStringValue(&NSString::from_str(filename));
    panel.setCanCreateDirectories(true);
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
        "md",
    )])));
    match panel.runModal() {
        response if response == NSModalResponseCancel => Ok(None),
        response if response == NSModalResponseOK => panel
            .URL()
            .and_then(|url| url.path())
            .map(|path| Some(std::path::PathBuf::from(path.to_string())))
            .ok_or_else(|| "Invalid export path".into()),
        _ => Err("macOS export dialog failed".into()),
    }
}

#[cfg(windows)]
fn save_dialog(owner: usize, text: &str, filename: &str) -> Result<Option<String>, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::HWND,
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::Controls::Dialogs::*,
        },
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let result = (|| {
        let mut name = vec![0u16; 4096];
        let initial: Vec<u16> = filename.encode_utf16().collect();
        name[..initial.len()].copy_from_slice(&initial);
        let filter: Vec<u16> = "Markdown transcript (*.md)\0*.md\0\0"
            .encode_utf16()
            .collect();
        let extension: Vec<u16> = "md\0".encode_utf16().collect();
        let title: Vec<u16> = "Export session transcript\0".encode_utf16().collect();
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND(owner as *mut _),
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(name.as_mut_ptr()),
            nMaxFile: name.len() as u32,
            lpstrDefExt: PCWSTR(extension.as_ptr()),
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !unsafe { GetSaveFileNameW(&mut dialog).as_bool() } {
            let error = unsafe { CommDlgExtendedError() };
            return if error.0 == 0 {
                Ok(None)
            } else {
                Err(format!("Windows export dialog failed: {}", error.0))
            };
        }
        let length = name
            .iter()
            .position(|c| *c == 0)
            .ok_or("Invalid export path")?;
        let path = PathBuf::from(OsString::from_wide(&name[..length]));
        write_atomic(&path, text)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })();
    unsafe {
        CoUninitialize();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_replaces_a_file_without_leaving_temporary_data() {
        let directory =
            std::env::temp_dir().join(format!("meeting-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.md");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, "Meeting speech\nMicrophone speech\nAI suggestion").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Meeting speech\nMicrophone speech\nAI suggestion"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
