pub mod core;

mod commands;
mod launch;
mod login_start;
mod shell_menu;
mod toolbar_window;
mod tray;
mod windows;

pub use crate::commands::shortcuts::ShortcutPlaces;

use crate::commands::settings::{ExplorerTexts, explorer_texts};
use crate::core::integration::autostart::{StartMode, start_mode};
use crate::core::model::Event;
use crate::core::{Core, Sink};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub(crate) type Shared = Arc<Core>;

/// Portable builds keep their data next to the executable; a `portable.txt`
/// marker file (shipped only in the portable zip) switches this on.
fn data_dir(app: &AppHandle) -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from))
        && dir.join("portable.txt").exists()
    {
        return dir.join("data");
    }
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("data"))
}

/// Registers every command the UI can call. The application and the
/// integration tests both build their app through this, so the tests exercise
/// the very same command dispatch as the real window.
pub fn configure<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    use commands::{addons, apps, console, devices, files, requests, scrcpy, sessions, settings, shortcuts, themes};
    builder
        .on_window_event(windows::on_window_event)
        // The files of the themes (their stylesheets, and the pictures and fonts of a theme of the user).
        .register_uri_scheme_protocol("theme", |context, request| {
            themes::serve(context.app_handle(), request.uri().path())
        })
        .invoke_handler(tauri::generate_handler![
        devices::get_snapshot,
        devices::pair_device,
        devices::connect_device,
        devices::forget_device,
        devices::set_device_label,
        devices::device_settings,
        devices::set_device_settings,
        devices::device_details,
        devices::device_infos,
        devices::refresh_device_info,
        devices::open_device_window,
        sessions::open_device,
        sessions::toolbar_buttons,
        sessions::toolbar_layout,
        sessions::press_button,
        apps::list_apps,
        apps::open_app,
        apps::cached_apps,
        apps::app_usage,
        apps::set_app_favorite,
        addons::device_menu,
        addons::run_menu_item,
        addons::app_menu,
        addons::run_app_menu_item,
        addons::choose_addon_folder,
        addons::list_addons,
        addons::inspect_addon_folder,
        addons::install_addon,
        addons::decide_addon_permissions,
        addons::uninstall_addon,
        addons::reload_addon,
        addons::set_addon_enabled,
        addons::addon_settings,
        addons::set_addon_setting,
        shortcuts::shortcuts_available,
        shortcuts::create_app_shortcut,
        shortcuts::app_shortcut_state,
        shortcuts::remove_app_shortcut,
        files::send_files,
        files::open_support,
        files::open_file_on_device,
        requests::current_request,
        requests::dismiss_request,
        requests::resolve_choice,
        requests::cancel_choice,
        settings::save_settings,
        settings::set_explorer_menu_texts,
        settings::explorer_menu_available,
        settings::launch_at_login_available,
        settings::detected_file_managers,
        settings::list_user_locales,
        settings::reveal_data_dir,
        scrcpy::scrcpy_versions,
        scrcpy::scrcpy_available,
        scrcpy::scrcpy_offer,
        scrcpy::scrcpy_download,
        scrcpy::scrcpy_use,
        console::open_debug_console,
        console::close_debug_console,
        console::open_console_window,
        themes::list_themes,
        themes::current_theme,
        themes::reveal_themes_dir,
        tray::set_tray_texts,
    ])
}

/// Where the core's events go: to the UI, and to the parts of the shell that
/// follow them (the tray's list of devices, the small window of a request).
fn event_sink(handle: AppHandle) -> Sink {
    Arc::new(move |event| {
        // The tray menu lists the devices: it follows the list.
        if let Event::State(snapshot) = &event {
            tray::refresh(&handle, &snapshot.devices);
        }
        // The small window of a request comes and goes with the request. Windows are
        // made from a task of their own, not from the thread that sends the event.
        if let Event::Request(view) = &event {
            let (app, shown) = (handle.clone(), view.is_some());
            tauri::async_runtime::spawn(async move {
                if shown { windows::show_request_window(&app) } else { windows::close_request_window(&app) }
            });
        }
        let _ = handle.emit("core-event", event);
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The first plugin: a second start must be told apart before anything else is made.
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        // Started again by the login while the program runs: there is nothing to do.
        if start_mode(&args) == StartMode::Background {
            return;
        }
        // Started again by hand: show the window that is there.
        if !launch::handle_arguments(app, &args) {
            windows::show_main(app);
        }
    }));
    configure(builder.plugin(tauri_plugin_opener::init()).plugin(tauri_plugin_dialog::init()))
        .setup(|app| {
            let handle = app.handle().clone();
            let core = Core::new(data_dir(&handle), event_sink(handle));
            app.manage(core.clone());
            app.manage(ExplorerTexts(std::sync::Mutex::new(Default::default())));
            app.manage(launch::ShellInbox::default());
            tray::setup(app)?;
            end_on_signals(app.handle().clone());
            tauri::async_runtime::spawn(core.clone().run());
            // The add-ons that are installed run their main.js; each may take a moment, so not here.
            let addons_core = core.clone();
            tauri::async_runtime::spawn_blocking(move || addons_core.addons().start_installed());
            // The menu of the Explorer follows the setting, and the path of this program.
            let settings = core.snapshot().settings;
            if let Err(error) =
                shell_menu::apply(settings.explorer_menu, &settings.disabled_file_managers, &explorer_texts(app.handle()))
            {
                eprintln!("the Explorer menu could not be brought up to date: {error}");
            }
            // The start at the login follows the setting, and the path of this program.
            if let Err(error) = login_start::apply(settings.launch_at_login) {
                eprintln!("the start at the login could not be brought up to date: {error}");
            }
            if let Some(places) = ShortcutPlaces::of_user(app.handle()) {
                app.manage(places);
            }
            // The window is made hidden: a start by the login stays in the tray, any other shows it.
            let args: Vec<String> = std::env::args().collect();
            if start_mode(&args) == StartMode::Window {
                windows::show_main(app.handle());
            }
            // Started by the Explorer menu or by a shortcut of an app: the request is the
            // arguments of this start.
            launch::handle_arguments(app.handle(), &args);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // The program ends (the tray's quit, the last window closed, a signal): the adb server that it
            // started is stopped. One that was running already is left to whoever started it.
            if let tauri::RunEvent::Exit = event
                && let Some(core) = app.try_state::<Shared>()
            {
                tauri::async_runtime::block_on(core.shutdown());
            }
        });
}

/// On Linux a logout or a `kill` ends the program with a signal, which would skip the end of the program; the
/// signals that ask it to end make it end as the tray's quit does. (Where there is no such signal, nothing is done.)
#[cfg(unix)]
fn end_on_signals(handle: AppHandle) {
    use tokio::signal::unix::{SignalKind, signal};
    for kind in [SignalKind::terminate(), SignalKind::interrupt(), SignalKind::hangup()] {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            // A signal that cannot be listened to is one that is not handled (the program ends by it as before).
            if let Ok(mut signal) = signal(kind) {
                signal.recv().await;
                handle.exit(0);
            }
        });
    }
}

#[cfg(not(unix))]
fn end_on_signals(_handle: AppHandle) {}
