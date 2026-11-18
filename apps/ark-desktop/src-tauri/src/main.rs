mod runtime;

use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_shell::ShellExt;

/// Native folder picker for the settings workspace field.
///
/// Resolves `None` when the user cancels. The dialog plugin dispatches to the
/// platform's main thread internally, so a plain channel handshake is enough.
/// The calling window must own the dialog: on Windows an unowned modal is
/// treated as a separate top-level window and can end up behind the main
/// window / only in the taskbar until the user clicks it.
#[tauri::command]
async fn pick_folder(
    app: tauri::AppHandle,
    window: tauri::Window,
) -> Result<Option<String>, String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .pick_folder(move |path| {
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

/// Post a system notification (Windows toast, macOS banner).
///
/// Session completions must reach the user while another app has focus, where
/// the in-page toast is invisible; this is the only surface that outlives the
/// WebView. The payload is display text only: the shell renders it verbatim
/// and takes no action on click, so no routing or IPC payload is modelled.
/// Delivery failures resolve to `false` instead of erroring — a lost
/// notification must never break the turn-end flow that reported it.
#[tauri::command]
fn notify_session_event(app: tauri::AppHandle, title: String, body: String) -> bool {
    // Blocked by the OS (notifications disabled) or no notification backend:
    // report failure so the renderer keeps its in-page toast as the fallback.
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .is_ok()
}

/// Open an `http(s)` URL in the OS default browser.
///
/// Prose links are model-authored text, so the scheme is re-validated here
/// rather than trusted from the renderer: `javascript:`, `data:` and `file:`
/// must never reach the OS handler, which would launch a program or read a
/// local file on the model's behalf. Handing the target to the OS is also the
/// only way a link can leave the window: an in-place navigation would replace
/// the app document with a page that has no route back (see `runtime.rs`).
#[tauri::command]
fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("refusing to open a {} url", parsed.scheme()));
    }
    app.shell()
        .open(parsed.as_str(), None)
        .map_err(|error| error.to_string())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .manage(runtime::ManagedRuntime::default())
        .invoke_handler(tauri::generate_handler![
            pick_folder,
            save_file,
            notify_session_event,
            open_external
        ])
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
