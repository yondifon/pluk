pub mod commands;
pub mod frame;
pub mod server;
#[cfg(target_os = "macos")]
mod tray_menu;
pub mod updater;
pub mod version;
pub mod zoom;

use std::sync::{Arc, Mutex};

use crate::commands::HostState;
use crate::server::ServerHandle;
use crate::updater::{Updater, UpdaterConfig};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use tauri::{
    Emitter, Manager, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};

const TRAY_ID: &str = "pluk-tray";
const TRAY_TOGGLE_ID: &str = "tray_toggle";
const TRAY_CHECK_UPDATES_ID: &str = "tray_updates";
const TRAY_QUIT_ID: &str = "tray_quit";
const CHECK_FOR_UPDATES_ID: &str = "check_for_updates";

#[tauri::command]
fn get_version() -> serde_json::Value {
    serde_json::json!({"version": version::version(),"commit": version::commit(),"commit_short": version::commit_short(),})
}

pub fn run() {
    let store = Arc::new(pluk_store::Store::open_default().expect("open pluk.db"));
    let sql_cancels = Arc::new(pluk_adapters::sql::SqlCancelRegistry::default());
    let registry = Arc::new(
        pluk_adapters::default_registry(store.clone(), sql_cancels.clone())
            .expect("register adapters"),
    );
    let zoom = Mutex::new(crate::zoom::PersistedZoom::load_from_store(&store));
    let server = tauri::async_runtime::block_on(async {
        ServerHandle::start_with_cancels(store.clone(), registry.clone(), sql_cancels.clone(), None)
            .await
            .expect("bind 4242")
    });
    let shared = server.state().clone();
    let host_state = HostState {
        store: store.clone(),
        server: tokio::sync::Mutex::new(server),
        shared,
        zoom,
    };
    let initial_zoom_title = {
        let z = host_state.zoom.lock().expect("zoom lock");
        z.state().reset_title()
    };
    let activity_store = store.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(host_state)
        .setup(move |app| {
            app.manage(Updater::new(UpdaterConfig::from_plugins(
                &app.config().plugins,
            )));
            // Every written log row reaches the window as it happens, so the
            // activity log needs no polling.
            let activity_app = app.handle().clone();
            activity_store.subscribe_log_activity(Arc::new(move |row| {
                if let Some(window) = activity_app.get_webview_window("main")
                    && let Ok(payload) = serde_json::to_value(row)
                {
                    let _ = window.emit("pluk://log-activity", payload);
                }
            }));
            // The status item carries no menu of its own: an attached menu is
            // opened by AppKit on either button, which would swallow the left
            // click. The right click attaches one for the length of the click.
            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button,
                        button_state,
                        ..
                    } = event
                        && button_state == tauri::tray::MouseButtonState::Down
                    {
                        match button {
                            tauri::tray::MouseButton::Left => toggle_window(tray.app_handle()),
                            #[cfg(target_os = "macos")]
                            tauri::tray::MouseButton::Right => tray_menu::show(tray.app_handle()),
                            _ => {}
                        }
                    }
                })
                .build(app)?;
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(ActivationPolicy::Accessory);
            }
            if let Some(window) = app.get_webview_window("main") {
                let f = frame::load(&frame::default_file_path());
                let _ = window.set_size(tauri::PhysicalSize::new(f.width as u32, f.height as u32));
                if let (Some(x), Some(y)) = (f.x, f.y) {
                    let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
                }
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_window(&app_handle);
                    }
                });
            }
            let app_menu = build_app_menu(app.handle(), &initial_zoom_title)?;
            app.set_menu(app_menu)?;
            app.on_menu_event(|app, event| match event.id.as_ref() {
                "zoom_in" => {
                    let state: tauri::State<HostState> = app.state();
                    let mut zoom = state.zoom.lock().expect("zoom lock");
                    zoom.state_mut().zoom_in();
                    let _ = zoom.save(Some(&state.store));
                    let scale = zoom.state().scale();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("pluk://zoom", scale);
                    }
                }
                "zoom_out" => {
                    let state: tauri::State<HostState> = app.state();
                    let mut zoom = state.zoom.lock().expect("zoom lock");
                    zoom.state_mut().zoom_out();
                    let _ = zoom.save(Some(&state.store));
                    let scale = zoom.state().scale();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("pluk://zoom", scale);
                    }
                }
                "zoom_reset" => {
                    let state: tauri::State<HostState> = app.state();
                    let mut zoom = state.zoom.lock().expect("zoom lock");
                    zoom.state_mut().reset();
                    let _ = zoom.save(Some(&state.store));
                    let scale = zoom.state().scale();
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.emit("pluk://zoom", scale);
                    }
                }
                CHECK_FOR_UPDATES_ID => {
                    tauri::async_runtime::spawn(updater::run_check(app.clone(), true));
                }
                TRAY_TOGGLE_ID => toggle_window(app),
                TRAY_CHECK_UPDATES_ID => {
                    // The check reports itself in the window, so bring it up.
                    show_window(app);
                    tauri::async_runtime::spawn(updater::run_check(app.clone(), true));
                }
                TRAY_QUIT_ID => app.exit(0),
                _ => {}
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                updater::run_check(handle.clone(), false).await;
                loop {
                    tokio::time::sleep(updater::CHECK_INTERVAL).await;
                    updater::run_check(handle.clone(), false).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            commands::get_zoom,
            commands::zoom_in,
            commands::zoom_out,
            commands::zoom_reset,
            commands::get_frame,
            commands::set_frame,
            commands::list_integrations,
            commands::get_integration,
            commands::create_integration,
            commands::update_integration,
            commands::delete_integration,
            commands::list_groups,
            commands::get_group,
            commands::create_group,
            commands::update_group,
            commands::delete_group,
            commands::list_adapters,
            commands::get_health,
            commands::test_connection,
            commands::get_logs,
            commands::get_retention,
            commands::set_retention,
            commands::clear_logs,
            commands::cancel_query,
            commands::reload,
            commands::inject_mcp_config,
            commands::list_installed_mcp_clients,
            updater::get_update_state,
            updater::check_for_updates,
            updater::install_update,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide_window(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("build tauri app")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let state: tauri::State<HostState> = app.state();
                tauri::async_runtime::block_on(async {
                    state.server.lock().await.stop().await;
                });
                if let Some(window) = app.get_webview_window("main")
                    && let Ok(pos) = window.outer_position()
                    && let Ok(size) = window.outer_size()
                {
                    let f = frame::Frame {
                        x: Some(pos.x as f64),
                        y: Some(pos.y as f64),
                        width: size.width as f64,
                        height: size.height as f64,
                    }
                    .clamped();
                    let _ = frame::save(&frame::default_file_path(), &f);
                }
            }
        });
}
/// The menu bar the platform expects: submenus off the root, standard items in
/// the standard places, so Undo/Cut/Copy/Paste and their accelerators reach the
/// webview and zoom sits under View.
fn build_app_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    zoom_reset_title: &str,
) -> tauri::Result<Menu<R>> {
    let check_updates = MenuItem::with_id(
        app,
        CHECK_FOR_UPDATES_ID,
        "Check for Updates…",
        true,
        None::<&str>,
    )?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit pluk"))?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &MenuItem::with_id(app, "zoom_in", "Zoom In", true, Some("CmdOrCtrl+Plus"))?,
            &MenuItem::with_id(app, "zoom_out", "Zoom Out", true, Some("CmdOrCtrl+-"))?,
            &MenuItem::with_id(app, "zoom_reset", zoom_reset_title, true, Some("CmdOrCtrl+0"))?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let menu = {
        let app_menu = Submenu::with_items(
            app,
            "pluk",
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &check_updates,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;
        let window = Submenu::with_items(
            app,
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(app, None)?,
                &PredefinedMenuItem::close_window(app, None)?,
            ],
        )?;
        Menu::with_items(app, &[&app_menu, &edit, &view, &window])?
    };
    #[cfg(not(target_os = "macos"))]
    let menu = {
        let file = Submenu::with_items(
            app,
            "File",
            true,
            &[
                &check_updates,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::minimize(app, None)?,
                &quit,
            ],
        )?;
        Menu::with_items(app, &[&file, &edit, &view])?
    };
    Ok(menu)
}
fn show_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(ActivationPolicy::Regular);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
fn hide_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(ActivationPolicy::Accessory);
    }
}
fn toggle_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let is_visible = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if is_visible {
        hide_window(app);
    } else {
        show_window(app);
    }
}
