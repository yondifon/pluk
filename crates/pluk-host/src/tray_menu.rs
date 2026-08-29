//! The status item's right-click menu on macOS.
//!
//! A menu left attached to the status item makes AppKit open it itself on
//! mouse-down, on either button, and the click never reaches the tray event
//! handler — so the left click can no longer toggle the window. The menu is
//! therefore attached for the length of one right click and detached again.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSStatusBarButton};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Manager, Runtime};

use crate::{TRAY_CHECK_UPDATES_ID, TRAY_ID, TRAY_QUIT_ID, TRAY_TOGGLE_ID};

/// Opens the menu under the status item. Runs on the main thread, after AppKit
/// is done with the click: tray events reach us through the event loop.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (Some(tray), Some(button)) = (app.tray_by_id(TRAY_ID), status_item_button(mtm)) else {
        return;
    };
    let Ok(menu) = build_menu(app) else {
        return;
    };
    if tray.set_menu(Some(menu)).is_err() {
        return;
    }
    // Clicking the button is what opens an attached menu at the status item;
    // it returns once the menu closes.
    unsafe { button.performClick(None) };
    let _ = tray.set_menu(None::<Menu<R>>);
    button.highlight(false);
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let showing = app
        .get_webview_window("main")
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    let toggle_title = if showing { "Hide pluk" } else { "Open pluk" };
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, TRAY_TOGGLE_ID, toggle_title, true, None::<&str>)?,
            &MenuItem::with_id(
                app,
                TRAY_CHECK_UPDATES_ID,
                "Check for Updates…",
                true,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, TRAY_QUIT_ID, "Quit pluk", true, None::<&str>)?,
        ],
    )
}

/// The tray crate keeps its `NSStatusItem` private, so the button is found
/// through the window AppKit gives it: the app owns exactly one status item.
fn status_item_button(mtm: MainThreadMarker) -> Option<Retained<NSStatusBarButton>> {
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find_map(|window| window.contentView()?.downcast::<NSStatusBarButton>().ok())
}
