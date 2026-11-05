//! Passes the system proxy to child programs as `HTTPS_PROXY`.
//! `ssh` and `ProxyCommand`s like `cloudflared` ignore the macOS system proxy,
//! and an app opened from Finder has no `HTTPS_PROXY` of its own.

use crate::platform::{self, SystemProxy};

/// When any of these is set, the user picked their own proxy.
const PROXY_VARS: &[&str] = &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];

const NO_PROXY_VARS: &[&str] = &["NO_PROXY", "no_proxy"];

/// Empty when no system proxy is on or the user already set a proxy variable.
pub fn system_proxy_env() -> Vec<(&'static str, String)> {
    let is_set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if PROXY_VARS.iter().any(|name| is_set(name)) {
        return Vec::new();
    }
    match platform::system_proxy() {
        Some(proxy) => proxy_env(&proxy, is_set),
        None => Vec::new(),
    }
}

/// The HTTPS proxy wins over SOCKS because more programs speak it.
fn proxy_env(proxy: &SystemProxy, is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    let url = match (&proxy.https, &proxy.socks) {
        (Some(address), _) => format!("http://{address}"),
        (None, Some(address)) => format!("socks5://{address}"),
        (None, None) => return Vec::new(),
    };
    let mut env = vec![("HTTPS_PROXY", url.clone()), ("https_proxy", url)];
    if !proxy.exceptions.is_empty() && !NO_PROXY_VARS.iter().any(|name| is_set(name)) {
        let hosts = proxy.exceptions.join(",");
        env.push(("NO_PROXY", hosts.clone()));
        env.push(("no_proxy", hosts));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(&str, String)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn socks_only_proxy_is_passed_as_a_socks_url() {
        let proxy = SystemProxy {
            socks: Some("127.0.0.1:7890".into()),
            ..Default::default()
        };
        let env = proxy_env(&proxy, |_| false);
        assert_eq!(value(&env, "HTTPS_PROXY"), Some("socks5://127.0.0.1:7890"));
    }

    #[test]
    fn user_no_proxy_is_kept() {
        let proxy = SystemProxy {
            https: Some("127.0.0.1:7890".into()),
            socks: Some("127.0.0.1:7891".into()),
            exceptions: vec!["*.local".into()],
        };
        let env = proxy_env(&proxy, |name| name == "NO_PROXY");
        assert_eq!(value(&env, "HTTPS_PROXY"), Some("http://127.0.0.1:7890"));
        assert_eq!(value(&env, "NO_PROXY"), None);
    }
}
