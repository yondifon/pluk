pub mod commands;
pub mod frame;
pub mod server;
pub mod version;
pub mod zoom;
#[allow(dead_code)]
pub mod updater;
use std::sync::{Arc, Mutex};
use tauri::{menu::{Menu, MenuItem, PredefinedMenuItem}, tray::TrayIconBuilder, Emitter, Manager, WindowEvent,};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use crate::commands::HostState;
use crate::server::ServerHandle;
#[tauri::command]
fn get_version() -> serde_json::Value { serde_json::json!({"version": version::version(),"commit": version::commit(),"commit_short": version::commit_short(),}) }
pub fn run() {
    let store = Arc::new(pluk_store::Store::open_default().expect("open pluk.db"));
    let registry = Arc::new(pluk_adapters::AdapterRegistry::new());
    let zoom = Mutex::new(crate::zoom::PersistedZoom::load_from_store(&store));
    let server = tauri::async_runtime::block_on(async { ServerHandle::start_default(store.clone(), registry.clone()).await.expect("bind 4242") });
    let host_state = HostState { store: store.clone(), server: tokio::sync::Mutex::new(server), zoom };
    let initial_zoom_title = { let z = host_state.zoom.lock().expect("zoom lock"); z.state().reset_title() };
    let updater = updater::Updater::new(updater::UpdaterConfig::placeholder());
    let builder = tauri::Builder::default();
    let _ = &updater;
    builder.manage(host_state).setup(move |app| {
            let show = MenuItem::with_id(app, "tray_show", "Open pluk", true, None::<&str>)?;
            let check_updates = MenuItem::with_id(app, "tray_updates", "Check for Updates…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "tray_quit", "Quit pluk", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show, &check_updates, &quit])?;
            let _tray = TrayIconBuilder::with_id("pluk-tray").icon(tauri::include_image!("icons/tray.png")).menu(&tray_menu).show_menu_on_left_click(false).on_tray_icon_event(|tray, event| { if let tauri::tray::TrayIconEvent::Click { button, .. } = event { if button == tauri::tray::MouseButton::Left { toggle_window(tray.app_handle()); } } }).on_menu_event(|app, event| match event.id.as_ref() { "tray_show" => toggle_window(app), "tray_quit" => app.exit(0), "tray_updates" => { show_window(app); } _ => {} }).build(app)?;
            #[cfg(target_os = "macos")] { let _ = app.set_activation_policy(ActivationPolicy::Accessory); }
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
                "zoom_in" => { let state: tauri::State<'_, HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().zoom_in(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } }
                "zoom_out" => { let state: tauri::State<'_, HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().zoom_out(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } }
                "zoom_reset" => { let state: tauri::State<'_, HostState> = app.state(); let mut zoom = state.zoom.lock().expect("zoom lock"); zoom.state_mut().reset(); let _ = zoom.save(Some(&state.store)); let scale = zoom.state().scale(); if let Some(window) = app.get_webview_window("main") { let _ = window.emit("pluk://zoom", scale); } }
                _ => {}
            });
            Ok(())
        }).invoke_handler(tauri::generate_handler![get_version, commands::get_zoom, commands::zoom_in, commands::zoom_out, commands::zoom_reset, commands::get_frame, commands::set_frame, commands::list_integrations, commands::get_integration, commands::create_integration, commands::update_integration, commands::delete_integration, commands::list_groups, commands::get_group, commands::create_group, commands::update_group, commands::delete_group, commands::list_adapters, commands::get_health, commands::test_connection, commands::get_logs, commands::cancel_query, commands::reload,]).on_window_event(|window, event| { if let WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); hide_window(window.app_handle()); } }).build(tauri::generate_context!()).expect("build tauri app").run(|app, event| { if let tauri::RunEvent::ExitRequested { .. } = event { let state: tauri::State<'_, HostState> = app.state(); tauri::async_runtime::block_on(async { state.server.lock().await.stop().await; }); if let Some(window) = app.get_webview_window("main") { if let Ok(pos) = window.outer_position() { if let Ok(size) = window.outer_size() { let f = frame::Frame { x: Some(pos.x as f64), y: Some(pos.y as f64), width: size.width as f64, height: size.height as f64 }.clamped(); let _ = frame::save(&frame::default_file_path(), &f); } } } } });
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
    let minimize = PredefinedMenuItem::minimize(app, None)?;
    Menu::with_items(app, &[&quit, &app_quit, &undo, &redo, &cut, &copy, &paste, &select_all, &zoom_in, &zoom_out, &zoom_reset, &minimize,])
}
fn show_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { #[cfg(target_os = "macos")] { let _ = app.set_activation_policy(ActivationPolicy::Regular); } if let Some(window) = app.get_webview_window("main") { let _ = window.show(); let _ = window.set_focus(); } }
fn hide_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); } #[cfg(target_os = "macos")] { let _ = app.set_activation_policy(ActivationPolicy::Accessory); } }
fn toggle_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) { let is_visible = app.get_webview_window("main").is_some_and(|w| w.is_visible().unwrap_or(false)); if is_visible { hide_window(app); } else { show_window(app); } }
