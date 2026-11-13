mod runtime;

use tauri_plugin_dialog::DialogExt;

/// Native folder picker for the settings workspace field.
///
/// Resolves `None` when the user cancels. The dialog plugin dispatches to the
/// platform's main thread internally, so a plain channel handshake is enough.
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = sender.send(path);
    });
    let picked = receiver.recv().map_err(|error| error.to_string())?;
    match picked {
        Some(path) => {
            let path = path.into_path().map_err(|error| error.to_string())?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        None => Ok(None),
    }
}

/// Persist bytes at an absolute path chosen via `pick_folder`.
///
/// The renderer only reaches this after a native save flow, and the shell
/// writes exactly the bytes it was given: no traversal tricks, no templating.
/// Returns the written path so the renderer can surface it.
#[tauri::command]
fn save_file(path: String, bytes: Vec<u8>) -> Result<String, String> {
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path)
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(runtime::ManagedRuntime::default())
        .invoke_handler(tauri::generate_handler![pick_folder, save_file])
        .setup(|app| {
            runtime::launch(app.handle())
                .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })
        })
        .build(tauri::generate_context!())
        .expect("ark desktop failed to start");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            runtime::shutdown(app);
        }
    });
}
