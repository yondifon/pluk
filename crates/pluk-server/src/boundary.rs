//! Who may reach the HTTP surface at all.
//!
//! Binding `127.0.0.1` keeps other machines out. It does not keep a web page
//! out: a cross-origin form POST needs no preflight, and any DNS name that
//! resolves to `127.0.0.1` turns a site into a same-origin client of this
//! server. So every request is pinned to a loopback `Host`, and a request a
//! browser labelled as coming from a page is refused.
//!
//! Agents, the CLI and the desktop window send neither `Origin` nor
//! `Sec-Fetch-Site`, and pass untouched. Two callers do speak like a browser
//! and have to keep working: the Chrome extension on `/wande`, which carries
//! its own extension origin and a pairing token, and the redirect landing on
//! the OAuth callback once the user has signed in on an upstream site.
//!
//! Nothing here answers with CORS headers. A refusal is one plain line, and
//! an allowed request is served exactly as it was before.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use pluk_adapters::mcp_proxy::oauth::CALLBACK_PATH;

/// The names this server answers to. Any port: the OS assigns it in tests and
/// `PORT` moves it in the app.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

pub(crate) async fn guard(request: Request, next: Next) -> Response {
    let loopback = header(request.headers(), "host")
        .is_some_and(|host| LOOPBACK_HOSTS.contains(&hostname(host)));
    if !loopback {
        return refuse("Pluk answers on the loopback host only.");
    }
    if !caller_allowed(&request) {
        return refuse("Pluk does not answer requests made by a web page.");
    }
    next.run(request).await
}

fn refuse(message: &'static str) -> Response {
    (StatusCode::FORBIDDEN, message).into_response()
}

/// Whether the client behind a request may be served. A client sending
/// neither `Origin` nor `Sec-Fetch-Site` is not a browser, which is every
/// agent, every CLI and the desktop app itself.
fn caller_allowed(request: &Request) -> bool {
    let headers = request.headers();
    let origin = header(headers, "origin");
    if is_extension_surface(request.uri().path()) {
        return origin.is_none_or(|origin| {
            origin.starts_with("chrome-extension://") || is_own_origin(origin, headers)
        });
    }
    match origin {
        Some(origin) => is_own_origin(origin, headers),
        None => match header(headers, "sec-fetch-site") {
            Some("cross-site" | "same-site") => is_oauth_return(request),
            _ => true,
        },
    }
}

/// `/wande` is the extension's surface. It pins the loopback port and the
/// extension origin itself and takes a pairing token on everything but its
/// health probe, so the origin refused here is only a web page's.
fn is_extension_surface(path: &str) -> bool {
    path == "/wande" || path.starts_with("/wande/")
}

/// The browser returning from an upstream sign-in page: a top-level `GET`
/// navigation from whichever site the user approved on. The single-use
/// `state` it carries is what ties it to a sign-in Pluk started.
fn is_oauth_return(request: &Request) -> bool {
    let headers = request.headers();
    request.method() == Method::GET
        && request.uri().path() == CALLBACK_PATH
        && header(headers, "sec-fetch-mode") == Some("navigate")
        && header(headers, "sec-fetch-dest") == Some("document")
}

/// This server's own origin, written any of the ways a loopback URL can be,
/// on the port the request arrived at.
fn is_own_origin(origin: &str, headers: &HeaderMap) -> bool {
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some(host) = header(headers, "host") else {
        return false;
    };
    LOOPBACK_HOSTS.contains(&hostname(authority)) && port(authority) == port(host)
}

/// An authority's host with the port dropped. An IPv6 literal keeps its
/// brackets, so it reads the same as the `Host` header wrote it.
fn hostname(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(rest) => match rest.find(']') {
            Some(end) => &authority[..end + 2],
            None => authority,
        },
        None => authority.split(':').next().unwrap_or(authority),
    }
}

/// An authority's port, empty when it was left implicit.
fn port(authority: &str) -> &str {
    authority[hostname(authority).len()..]
        .strip_prefix(':')
        .unwrap_or_default()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}
