//! The environment a program the user named expects to start in.
//!
//! An app opened from Finder inherits launchd's `PATH`, which has no Homebrew,
//! bun or uv, so the login shell's `PATH` is read once and used for both.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::process::run_capture;

/// How long the login shell gets to print its `PATH`.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// Fences the `PATH` off from whatever the user's dotfiles print.
const MARKER: &str = "__PLUK_PATH__";

/// Where the usual installers put programs, for when the login shell cannot
/// be read.
const FALLBACK_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.bun/bin",
    "~/.local/bin",
    "~/.cargo/bin",
    "~/.volta/bin",
    "~/.deno/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// All a child gets from Pluk's own environment, besides `PATH`.
const PASSED_THROUGH: &[&str] = &["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG"];

/// Read once; the usual install folders stand in when the shell cannot be read.
pub async fn login_path() -> &'static OsStr {
    static PATH: OnceCell<OsString> = OnceCell::const_new();
    PATH.get_or_init(|| async {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/zsh"));
        read_login_path(Path::new(&shell), SHELL_TIMEOUT)
            .await
            .unwrap_or_else(fallback_path)
    })
    .await
}

pub async fn read_login_path(shell: &Path, timeout: Duration) -> Option<OsString> {
    let mut command = Command::new(shell);
    command.args([
        "-l",
        "-i",
        "-c",
        &format!("printf '\\n{MARKER}%s{MARKER}' \"$PATH\""),
    ]);
    let captured = run_capture(&mut command, timeout).await.ok()?;
    let stdout = String::from_utf8_lossy(&captured.stdout);
    let (_, rest) = stdout.rsplit_once(MARKER)?.0.rsplit_once(MARKER)?;
    let path = rest.trim();
    (!path.is_empty()).then(|| OsString::from(path))
}

pub fn fallback_path() -> OsString {
    let dirs: Vec<PathBuf> = FALLBACK_DIRS.iter().map(|dir| expand_home(dir)).collect();
    std::env::join_paths(dirs).unwrap_or_default()
}

pub fn expand_home(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if path == "~" {
        if let Some(home) = home() {
            return home;
        }
    } else if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// A relative path with a `/` is refused: it would depend on the folder Pluk
/// happens to run in. A bare name is looked up in `path`.
pub fn resolve_program(command: &str, path: &OsStr) -> io::Result<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no command given",
        ));
    }
    let expanded = expand_home(command);
    if expanded.is_absolute() {
        return if is_executable(&expanded) {
            Ok(expanded)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "not an executable file",
            ))
        };
    }
    if command.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a relative path depends on the folder Pluk runs in",
        ));
    }
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not found on the PATH"))
}

pub fn base_env(path: &OsStr) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| {
            name.to_str()
                .is_some_and(|name| PASSED_THROUGH.contains(&name) || name.starts_with("LC_"))
        })
        .collect();
    env.push((OsString::from("PATH"), path.to_os_string()));
    env
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn fake_shell(dir: &Path) -> PathBuf {
        script(
            dir,
            "fake-shell",
            "echo 'Welcome back'\nPATH=/fake/bin:/usr/bin\nshift 3\neval \"$1\"\necho 'bye'",
        )
    }

    #[tokio::test]
    async fn the_login_path_is_read_between_its_markers() {
        let dir = tempfile::tempdir().unwrap();
        let path = read_login_path(&fake_shell(dir.path()), Duration::from_secs(5)).await;
        assert_eq!(path, Some(OsString::from("/fake/bin:/usr/bin")));
    }

    #[tokio::test]
    async fn a_shell_that_fails_or_hangs_gives_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let failing = script(dir.path(), "failing", "echo nope; exit 1");
        assert_eq!(
            read_login_path(&failing, Duration::from_secs(5)).await,
            None
        );
        let hanging = script(dir.path(), "hanging", "sleep 30");
        assert_eq!(
            read_login_path(&hanging, Duration::from_millis(300)).await,
            None
        );
        assert_eq!(
            read_login_path(Path::new("/no/such/shell"), Duration::from_secs(5)).await,
            None
        );
    }

    #[test]
    fn the_fallback_holds_the_usual_install_folders() {
        let fallback = fallback_path();
        let dirs: Vec<PathBuf> = std::env::split_paths(&fallback).collect();
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(dirs.contains(&home.join(".bun/bin")));
        assert!(dirs.iter().all(|dir| dir.is_absolute()), "{dirs:?}");
    }

    #[test]
    fn a_bare_name_is_found_on_the_path_and_spawned_by_its_full_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = script(&bin, "my-server", "exit 0");
        std::fs::write(bin.join("not-executable"), "").unwrap();
        let path = std::env::join_paths(["/nowhere", bin.to_str().unwrap()]).unwrap();

        assert_eq!(resolve_program("my-server", &path).unwrap(), tool);
        assert_eq!(
            resolve_program("not-executable", &path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            resolve_program("missing", &path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn an_absolute_path_must_be_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = script(dir.path(), "server", "exit 0");
        let path = OsString::new();
        assert_eq!(
            resolve_program(tool.to_str().unwrap(), &path).unwrap(),
            tool
        );
        assert!(resolve_program(dir.path().to_str().unwrap(), &path).is_err());
        assert!(resolve_program("/no/such/program", &path).is_err());
    }

    #[test]
    fn a_relative_path_is_refused_even_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "server", "exit 0");
        let path = std::env::join_paths([dir.path()]).unwrap();
        for relative in ["./server", "bin/server", "../server"] {
            assert_eq!(
                resolve_program(relative, &path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{relative}"
            );
        }
        assert!(resolve_program("  ", &path).is_err());
    }

    #[test]
    fn home_is_expanded_only_at_the_start() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/code/mcp"), home.join("code/mcp"));
        assert_eq!(expand_home("/srv/~/x"), PathBuf::from("/srv/~/x"));
        assert_eq!(expand_home("~other"), PathBuf::from("~other"));
    }

    #[test]
    fn a_child_gets_the_short_list_and_the_resolved_path() {
        let env = base_env(OsStr::new("/fake/bin"));
        let names: Vec<&str> = env.iter().filter_map(|(name, _)| name.to_str()).collect();
        assert!(names.contains(&"HOME"), "{names:?}");
        assert!(
            names.iter().all(|name| PASSED_THROUGH.contains(name)
                || name.starts_with("LC_")
                || *name == "PATH"),
            "{names:?}"
        );
        assert_eq!(
            env.last(),
            Some(&(OsString::from("PATH"), OsString::from("/fake/bin")))
        );
    }
}
