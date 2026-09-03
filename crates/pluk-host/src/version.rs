//! Version and commit stamping — baked at bundle time.
//!
//! `build.rs` reads `VERSION` and `git rev-parse HEAD` and exposes them as
//! compile-time env vars `PLUK_VERSION` / `PLUK_COMMIT`. They are available to
//! the running app via `version()` / `commit()` and via the Tauri command
//! `get_version` (used by the frontend for bug reports and the updater).

/// Version string from `VERSION` file at build time (falls back to Cargo version).
pub fn version() -> &'static str {
    option_env!("PLUK_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Full git commit hash at build time, or "unknown" in dev without git.
pub fn commit() -> &'static str {
    option_env!("PLUK_COMMIT").unwrap_or("unknown")
}

/// Short 7-char commit for display.
pub fn commit_short() -> &'static str {
    option_env!("PLUK_COMMIT_SHORT").unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
    #[test]
    fn commit_is_non_empty() {
        assert!(!commit().is_empty());
    }
}
