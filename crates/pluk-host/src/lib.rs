pub mod commands;
pub mod frame;
pub mod server;
pub mod version;
pub mod zoom;
pub mod updater;

use std::sync::{Arc, Mutex};

use tauri::{menu::{Menu, MenuItem, PredefinedMenuItem}, tray::TrayIconBuilder, Emitter, Manager, WindowEvent,};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use crate::commands::HostState;
use crate::server::ServerHandle;
use crate::updater::{Updater, UpdaterConfig};

#[tauri::command]
fn get_version() -> serde_json::Value { serde_json::json!({"version": version::version(),"commit": version::commit(),"commit_short": version::commit_short(),}) }

pub fn run() {
    let store = Arc::new(pluk_store::Store::open_default().expect("open pluk.db"));
    let sql_cancels = Arc::new(pluk_adapters::sql::SqlCancelRegistry::default());
    let registry = Arc::new(
        pluk_adapters::default_registry(store.clone(), sql_cancels.clone()).expect("register adapters"),
    );
    let zoom = Mutex::new(crate::zoom::PersistedZoom::load_from_store(&store));
    let server = tauri::async_runtime::block_on(async { ServerHandle::start_with_cancels(store.clone(), registry.clone(), sql_cancels.clone(), None).await.expect("bind 4242") });
    let shared = server.state().clone();
    let host_state = HostState { store: store.clone(), server: tokio::sync::Mutex::new(server), shared, zoom };
    let initial_zoom_title = { let z = host_state.zoom.lock().expect("zoom lock"); z.state().reset_title() };
    let updater = Updater::new(UpdaterConfig::placeholder());
    let activity_store = store.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(host_state)
        .manage(updater.clone())
        .setup(move |app| {
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
            let show = MenuItem::with_id(app, "tray_show", "Open pluk", true, None::<&str>)?;
            let check_updates = MenuItem::with_id(app, "tray_updates", "Check for Updates…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "tray_quit", "Quit pluk", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show, &check_updates, &quit])?;
            let updater_for_tray = updater.clone();
            let _tray = TrayIconBuilder::with_id("pluk-tray").icon(tauri::include_image!("icons/tray.png")).menu(&tray_menu).show_menu_on_left_click(false).on_tray_icon_event(|tray, event| { if let tauri::tray::TrayIconEvent::Click { button, .. } = event && button == tauri::tray::MouseButton::Left { toggle_window(tray.app_handle()); } }).on_menu_event(move |app, event| match event.id.as_ref() {
                "tray_show" => toggle_window(app),
                "tray_quit" => app.exit(0),
                "tray_updates" => {
                    show_window(app);
                    if let Some(u) = app.try_state::<Updater>() {
                        if u.is_configured() && u.begin_check() {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.emit("pluk://update-state", serde_json::to_value(u.state()).unwrap());
                            }
                        } else if let Some(w) = app.get_webview_window("main") {
                            let _ = w.emit("pluk://update-state", serde_json::to_value(u.state()).unwrap());
                        }
                    } else if updater_for_tray.is_configured() && updater_for_tray.begin_check() && let Some(w) = app.get_webview_window("main") {
                        let _ = w.emit("pluk://update-state", serde_json::to_value(updater_for_tray.state()).unwrap());
                    }
                },
                _ => {}
            }).build(app)?;
            #[cfg(target_os = "macos")] { app.set_activation_policy(ActivationPolicy::Accessory); }
            if let Some(window) = app.get_webview_window("main") {
                let f = frame::load(&frame::default_file_path());
                let _ = window.set_size(tauri::PhysicalSize::new(f.width as u32, f.height as u32));
                if let (Some(x), Some(y)) = (f.x, f.y) { let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32)); }
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| { if let WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); hide_window(&app_handle); } });
            }
            let app_menu = build_app_menu(app.handle(), &initial_zoom_title)?;
            app.set_menu(app_menu)?;
            app.on_menu_event(|app, event| match event.id.as_ref() {
                "zoom_in" => { let state: tauri::State<HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().zoom_in(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } },
                "zoom_out" => { let state: tauri::State<HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().zoom_out(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } },
                "zoom_reset" => { let state: tauri::State<HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().reset(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } },
                "check_for_updates" => {
                    if let Some(u) = app.try_state::<Updater>()
                        && u.is_configured() && u.begin_check()
                            && let Some(w) = app.get_webview_window("main") {
                                let _ = w.emit("pluk://update-state", serde_json::to_value(u.state()).unwrap());
                            }
                },
                _ => {}
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(updater::CHECK_INTERVAL).await;
                    if let Some(u) = handle.try_state::<Updater>() {
                        if !u.is_configured() { continue; }
                        if u.begin_check() {
                            if let Some(w) = handle.get_webview_window("main") {
                                let _ = w.emit("pluk://update-state", serde_json::to_value(u.state()).unwrap());
                            }
                            let h2 = handle.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                                if let Some(u2) = h2.try_state::<Updater>() && matches!(u2.state(), updater::UpdateState::Checking) {
                                    u2.set_state(updater::UpdateState::Idle);
                                    if let Some(w) = h2.get_webview_window("main") {
                                        let _ = w.emit("pluk://update-state", serde_json::to_value(u2.state()).unwrap());
                                    }
                                }
                            });
                        }
                    }
                }
            });
            Ok(())
        }).invoke_handler(tauri::generate_handler![get_version, commands::get_zoom, commands::zoom_in, commands::zoom_out, commands::zoom_reset, commands::get_frame, commands::set_frame, commands::list_integrations, commands::get_integration, commands::create_integration, commands::update_integration, commands::delete_integration, commands::list_groups, commands::get_group, commands::create_group, commands::update_group, commands::delete_group, commands::list_adapters, commands::get_health, commands::test_connection, commands::get_logs, commands::get_retention, commands::set_retention, commands::clear_logs, commands::cancel_query, commands::reload, commands::inject_mcp_config, commands::list_installed_mcp_clients, updater::get_update_state, updater::check_for_updates, updater::install_update,]).on_window_event(|window, event| { if let WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); hide_window(window.app_handle()); } }).build(tauri::generate_context!()).expect("build tauri app").run(|app, event| { if let tauri::RunEvent::ExitRequested { .. } = event { let state: tauri::State<HostState> = app.state(); tauri::async_runtime::block_on(async { state.server.lock().await.stop().await; }); if let Some(window) = app.get_webview_window("main") && let Ok(pos) = window.outer_position() && let Ok(size) = window.outer_size() { let f = frame::Frame { x: Some(pos.x as f64), y: Some(pos.y as f64), width: size.width as f64, height: size.height as f64 }.clamped(); let _ = frame::save(&frame::default_file_path(), &f); } } });
}
fn build_app_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>, zoom_reset_title: &str) -> tauri::Result<Menu<R>> {
    let quit = PredefinedMenuItem::quit(app, Some("Quit pluk"))?;
    let app_quit = MenuItem::with_id(app, "app_quit", "Quit pluk", true, None::<&str>)?;
    let undo = PredefinedMenuItem::undo(app, None)?;
    let redo = PredefinedMenuItem::redo(app, None)?;
    let cut = PredefinedMenuItem::cut(app, None)?;
    let copy = PredefinedMenuItem::copy(app, None)?;
    let paste = PredefinedMenuItem::paste(app, None)?;
    let select_all = PredefinedMenuItem::select_all(app, None)?;
    let zoom_in = MenuItem::with_id(app, "zoom_in", "Zoom In", true, Some("CmdOrCtrl+Plus"))?;
    let zoom_out = MenuItem::with_id(app, "zoom_out", "Zoom Out", true, Some("CmdOrCtrl+-"))?;
    let zoom_reset = MenuItem::with_id(app, "zoom_reset", zoom_reset_title, true, Some("CmdOrCtrl+0"))?;
    let check_updates = MenuItem::with_id(app, "check_for_updates", "Check for Updates…", true, None::<&str>)?;
    let minimize = PredefinedMenuItem::minimize(app, None)?;
    Menu::with_items(app, &[&quit, &app_quit, &undo, &redo, &cut, &copy, &paste, &select_all, &zoom_in, &zoom_out, &zoom_reset, &check_updates, &minimize,])
}
fn show_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { #[cfg(target_os = "macos")] { let _ = app.set_activation_policy(ActivationPolicy::Regular); } if let Some(window) = app.get_webview_window("main") { let _ = window.show(); let _ = window.set_focus(); } }
fn hide_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); } #[cfg(target_os = "macos")] { let _ = app.set_activation_policy(ActivationPolicy::Accessory); } }
fn toggle_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { let is_visible = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false)); if is_visible { hide_window(app); } else { show_window(app); } }
