use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::ManagerExt;

const DEFAULT_APPLIED_KEY: &str = "open_at_login_default_applied";

pub fn is_enabled<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    let registered = app.autolaunch().is_enabled().map_err(|e| e.to_string())?;
    if !registered {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/launchctl")
            .args(["print-disabled", &launch_domain()])
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(
            !String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                line.trim().split_once("=>").is_some_and(|(label, value)| {
                    label.trim() == "\"Pluk\"" && value.trim() == "true"
                })
            }),
        )
    }
    #[cfg(not(target_os = "macos"))]
    Ok(registered)
}

#[cfg(target_os = "macos")]
fn launch_domain() -> String {
    // getuid has no preconditions and returns the current user's launchd domain.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn enable<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    app.autolaunch().enable().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/launchctl")
            .args(["enable", &format!("{}/Pluk", launch_domain())])
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
    }
    Ok(())
}

fn should_apply_default(release: bool, applied: Option<&str>) -> bool {
    release && applied.is_none()
}

pub fn apply_default<R: Runtime>(
    app: &AppHandle<R>,
    store: &pluk_store::Store,
) -> Result<(), String> {
    let applied = store
        .get_setting(DEFAULT_APPLIED_KEY)
        .map_err(|e| e.to_string())?;
    if should_apply_default(cfg!(not(debug_assertions)), applied.as_deref()) {
        // Persist before registering: a failed write must not allow a later opt-out to be lost.
        store
            .set_setting(DEFAULT_APPLIED_KEY, "true")
            .map_err(|e| e.to_string())?;
        enable(app)?;
    }
    Ok(())
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let result = (|| -> Result<(), String> {
        let enabled = is_enabled(app)?;
        let state = app.state::<crate::commands::HostState>();
        state
            .store
            .set_setting(DEFAULT_APPLIED_KEY, "true")
            .map_err(|e| e.to_string())?;
        if enabled {
            app.autolaunch().disable().map_err(|e| e.to_string())
        } else {
            enable(app)
        }
    })();
    if let Err(reason) = result {
        crate::show_error(
            app,
            format!("Pluk can't change Open at Login. Try again from the menu bar.\n\n{reason}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_out_survives_reopening_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pluk.db");
        let store = pluk_store::Store::open(&path).unwrap();
        assert!(should_apply_default(
            true,
            store.get_setting(DEFAULT_APPLIED_KEY).unwrap().as_deref()
        ));
        store.set_setting(DEFAULT_APPLIED_KEY, "true").unwrap();
        drop(store);
        let store = pluk_store::Store::open(&path).unwrap();
        assert!(!should_apply_default(
            true,
            store.get_setting(DEFAULT_APPLIED_KEY).unwrap().as_deref()
        ));
    }

    #[test]
    fn development_launch_does_not_consume_the_default() {
        assert!(!should_apply_default(false, None));
        assert!(should_apply_default(true, None));
    }
}
