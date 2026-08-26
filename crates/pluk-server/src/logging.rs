//! Append-only debug log at `<data_dir>/pluk.log`, mirrored to stderr.
//!
//! Ported from `pluk/src/log.ts`. Logging must never break a request: every
//! write is best-effort.

use std::io::Write;

use pluk_core::platform;

pub fn log_path() -> std::path::PathBuf {
    platform::log_file()
}

fn write(level: &str, msg: &str, meta: Option<serde_json::Value>) {
    let meta_str = match meta {
        Some(value) if value.as_object().is_some_and(|o| !o.is_empty()) => {
            format!(" {value}")
        }
        _ => String::new(),
    };
    let line = format!("{} [{level}] {msg}{meta_str}", pluk_store::timestamp::now_utc_string());
    let _ = append_line(&line);
    eprintln!("{line}");
}

fn append_line(line: &str) -> std::io::Result<()> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

pub fn log_info(msg: &str) {
    write("info", msg, None);
}

pub fn log_error(msg: &str, error: &dyn std::fmt::Display, meta: Option<serde_json::Value>) {
    let mut object = match meta {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    object.insert("error".into(), serde_json::Value::String(error.to_string()));
    write("error", msg, Some(serde_json::Value::Object(object)));
}
