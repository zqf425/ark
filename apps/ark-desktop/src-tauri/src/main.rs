mod runtime;

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(runtime::ManagedRuntime::default())
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
