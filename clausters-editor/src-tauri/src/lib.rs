mod books;
mod cli;
mod config;
mod envs;
mod files;
mod fonts;
mod pkgdocs;
mod session;
#[cfg(target_os = "linux")]
mod window_size;

use tauri::{Manager, RunEvent};

use config::ConfigState;
use session::Session;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything opens: `--help`, `--version` and a bad argument print and
    // exit here, with no window.
    let cli = <cli::Cli as clap::Parser>::parse();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // The window comes back where it was left: its size, its place on the
        // screen, maximized or not. The panels inside it are the page's own
        // settings (settings.svelte.ts). On Linux the size is kept by
        // `window_size` instead, which says why.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags())
                .build(),
        )
        .manage(Session::default())
        .manage(cli)
        .setup(|app| {
            let config = config::load(app.handle());
            app.manage(ConfigState(std::sync::Mutex::new(config)));
            #[cfg(target_os = "linux")]
            if let Some(window) = app.get_webview_window("main") {
                window_size::manage(&window);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cli::cli_files,
            session::py_eval,
            session::py_query,
            session::py_restart,
            session::py_interrupt,
            books::list_books,
            files::list_docs,
            files::read_doc,
            files::search_docs,
            files::read_file,
            files::write_file,
            fonts::list_mono_fonts,
            config::get_config,
            config::set_docs_dir,
            config::set_startup_code,
            envs::list_envs,
            envs::create_env,
            envs::install_in_env,
            envs::delete_env,
            envs::use_env,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting the application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<Session>().kill();
            }
        });
}

/// What the window-state plugin keeps: everything, but the size on Linux.
fn window_state_flags() -> tauri_plugin_window_state::StateFlags {
    let all = tauri_plugin_window_state::StateFlags::all();
    if cfg!(target_os = "linux") {
        all - tauri_plugin_window_state::StateFlags::SIZE
    } else {
        all
    }
}
