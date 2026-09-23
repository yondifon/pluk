//! The headers Pluk sends an upstream server on top of its sign-in.
//!
//! The integration's `headers` rows are sent on every request of a session,
//! whatever the sign-in: none, a saved token, or an OAuth sign-in. Values go
//! out exactly as the user wrote them. A secret row's value is read from
//! `proxy_secrets` and is only ever held as a [`Secret`].
//!
//! A few names are refused because the transport sets them itself, or
//! because they frame the request. `Authorization` is only taken when the
//! integration has no other sign-in; when it later gains one, the sign-in
//! wins and the row is not sent.

use sha2::{Digest, Sha256};
use upstream_http::header::{AUTHORIZATION, HeaderName, HeaderValue};

use pluk_store::{Integration, ProxySecret, SecretKind, Store};

use crate::error::AdapterError;
use crate::key_value::{self, Row};

/// The config key the header rows are kept under.
pub const HEADERS_KEY: &str = "headers";

/// Names the transport sets itself or that frame the request, lowercase. Any
/// name starting with `mcp-` is refused as well.
const RESERVED: &[&str] = &[
    "accept",
    "content-type",
    "content-length",
    "transfer-encoding",
    "connection",
    "host",
    "cookie",
    "last-event-id",
];

const AUTHORIZATION_WITH_TOKEN: &str =
    "Authorization cannot be added while a token is saved. Remove the token or this header.";
const AUTHORIZATION_WITH_SIGN_IN: &str =
    "Authorization is already sent by the sign-in. Remove this header.";

/// A value that never renders. There is no `Display` or `Serialize`, and
/// `Debug` prints `<redacted>`; [`Secret::expose`] is called only where a
/// header value is built or hashed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderText {
    Plain(String),
    Secret(Secret),
}

/// One header sent on every request, as the user wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticHeader {
    pub name: HeaderName,
    pub value: HeaderText,
}

impl StaticHeader {
    pub fn is_secret(&self) -> bool {
        matches!(self.value, HeaderText::Secret(_))
    }

    /// The value to put on the wire. A secret one is marked sensitive, so
    /// the HTTP stack keeps it out of its own debug output.
    pub fn header_value(&self) -> Result<HeaderValue, AdapterError> {
        let (text, sensitive) = match &self.value {
            HeaderText::Plain(text) => (text.as_str(), false),
            HeaderText::Secret(secret) => (secret.expose(), true),
        };
        let mut value = HeaderValue::from_str(text)
            .map_err(|_| AdapterError::new(bad_value(self.name.as_str())))?;
        value.set_sensitive(sensitive);
        Ok(value)
    }
}

/// Which sign-in an integration already sends. `Authorization` is taken only
/// without one, and a row cannot reuse the token's own header.
pub enum SignIn {
    None,
    /// A saved token, sent in the named header.
    Token {
        header: String,
    },
    /// An OAuth sign-in, sent as `Authorization`.
    Oauth,
}

/// The first header row a save would refuse: its position and why. Blank
/// rows are skipped; missing names and values are the row list's own check.
pub fn check_rows(rows: &[Row], sign_in: &SignIn) -> Result<(), (usize, String)> {
    for (index, row) in rows.iter().enumerate() {
        if row.name.is_empty() {
            continue;
        }
        let name = parse_name(&row.name).map_err(|message| (index, message))?;
        if name == AUTHORIZATION {
            match sign_in {
                SignIn::None => {}
                SignIn::Token { .. } => return Err((index, AUTHORIZATION_WITH_TOKEN.to_string())),
                SignIn::Oauth => return Err((index, AUTHORIZATION_WITH_SIGN_IN.to_string())),
            }
        } else if let SignIn::Token { header } = sign_in
            && name.as_str().eq_ignore_ascii_case(header.trim())
        {
            return Err((
                index,
                format!(
                    "{} is already sent with the token. Remove this header.",
                    row.name
                ),
            ));
        }
        if !row.value.is_empty() && HeaderValue::from_str(&row.value).is_err() {
            return Err((index, bad_value(&row.name)));
        }
    }
    Ok(())
}

/// Every header row of the integration, secret values read from the store.
/// A secret row with nothing saved is skipped.
pub fn static_headers(
    store: &Store,
    conn: &Integration,
) -> Result<Vec<StaticHeader>, AdapterError> {
    let rows = key_value::rows(&conn.config, HEADERS_KEY);
    if !rows.iter().any(|row| row.secret) {
        return headers_of(&rows, &[]);
    }
    let saved = store
        .list_proxy_secrets(&conn.id)
        .map_err(|error| AdapterError::new(error.to_string()))?;
    headers_of(&rows, &saved)
}

/// Only the plain header rows, which carry routing data rather than
/// credentials.
pub fn plain_headers(conn: &Integration) -> Result<Vec<StaticHeader>, AdapterError> {
    let rows = key_value::rows(&conn.config, HEADERS_KEY);
    let plain: Vec<Row> = rows.into_iter().filter(|row| !row.secret).collect();
    headers_of(&plain, &[])
}

/// Whether a saved secret header goes out with every request, which counts
/// as a sign-in the user already handed over.
pub fn has_secret_headers(store: &Store, conn: &Integration) -> Result<bool, AdapterError> {
    Ok(static_headers(store, conn)?
        .iter()
        .any(StaticHeader::is_secret))
}

/// A digest of the headers, so a change to any name or value reads as a
/// change without the values being kept around.
pub fn digest(headers: &[StaticHeader]) -> String {
    let mut hasher = Sha256::new();
    for header in headers {
        let text = match &header.value {
            HeaderText::Plain(text) => text.as_str(),
            HeaderText::Secret(secret) => secret.expose(),
        };
        hasher.update(header.name.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(text.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn headers_of(rows: &[Row], saved: &[ProxySecret]) -> Result<Vec<StaticHeader>, AdapterError> {
    let mut headers = Vec::new();
    for row in rows {
        if row.name.is_empty() {
            continue;
        }
        let name = parse_name(&row.name).map_err(AdapterError::new)?;
        let value = if row.secret {
            let saved = saved
                .iter()
                .find(|secret| secret.kind == SecretKind::Header && secret.name == row.name);
            match saved {
                Some(secret) => HeaderText::Secret(Secret::new(secret.value.clone())),
                None => continue,
            }
        } else {
            HeaderText::Plain(row.value.clone())
        };
        let header = StaticHeader { name, value };
        header.header_value()?;
        headers.push(header);
    }
    Ok(headers)
}

fn parse_name(name: &str) -> Result<HeaderName, String> {
    let parsed = HeaderName::try_from(name).map_err(|_| {
        format!("{name} is not a valid header name. Use letters, numbers and dashes.")
    })?;
    let lower = parsed.as_str();
    if RESERVED.contains(&lower) || lower.starts_with("mcp-") {
        return Err(format!("Pluk cannot send {name}. Remove this header."));
    }
    Ok(parsed)
}

fn bad_value(name: &str) -> String {
    format!("The value for {name} has characters a header cannot carry. Check for line breaks.")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::mcp_proxy::tests::integration;

    fn row(name: &str, value: &str) -> Row {
        Row {
            name: name.to_string(),
            value: value.to_string(),
            secret: true,
            saved_name: None,
        }
    }

    fn refusal(rows: &[Row], sign_in: SignIn) -> String {
        check_rows(rows, &sign_in).expect_err("refused").1
    }

    #[test]
    fn names_pluk_cannot_send_are_refused_in_any_case() {
        for name in [
            "Accept",
            "content-type",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "HOST",
            "Cookie",
            "Last-Event-Id",
            "Mcp-Session-Id",
            "MCP-Protocol-Version",
            "mcp-anything",
        ] {
            assert_eq!(
                refusal(&[row(name, "v")], SignIn::None),
                format!("Pluk cannot send {name}. Remove this header."),
            );
        }
        assert!(
            check_rows(
                &[row("DD_API_KEY", "k"), row("wix-account-id", "a")],
                &SignIn::None
            )
            .is_ok()
        );
    }

    #[test]
    fn a_bad_name_or_value_is_named_and_the_value_never_shown() {
        assert_eq!(
            check_rows(&[row("X-Ok", "v"), row("Bad Name", "v")], &SignIn::None),
            Err((
                1,
                "Bad Name is not a valid header name. Use letters, numbers and dashes.".to_string()
            ))
        );
        let refused = refusal(&[row("X-Key", "line\nbreak-secret")], SignIn::None);
        assert_eq!(
            refused,
            "The value for X-Key has characters a header cannot carry. Check for line breaks."
        );
        assert!(!refused.contains("break-secret"));
    }

    #[test]
    fn authorization_is_taken_only_without_another_sign_in() {
        let authorization = [row("Authorization", "IST.wix-key")];
        assert!(check_rows(&authorization, &SignIn::None).is_ok());
        for header in ["Authorization", "X-Api-Key"] {
            let token = SignIn::Token {
                header: header.to_string(),
            };
            assert_eq!(refusal(&authorization, token), AUTHORIZATION_WITH_TOKEN);
        }
        assert_eq!(
            refusal(&authorization, SignIn::Oauth),
            AUTHORIZATION_WITH_SIGN_IN
        );
        assert_eq!(
            refusal(
                &[row("x-api-key", "v")],
                SignIn::Token {
                    header: "X-Api-Key".to_string()
                }
            ),
            "x-api-key is already sent with the token. Remove this header."
        );
    }

    #[test]
    fn a_secret_prints_as_redacted() {
        let header = StaticHeader {
            name: HeaderName::from_static("dd-api-key"),
            value: HeaderText::Secret(Secret::new("dd-secret-1")),
        };
        let printed = format!("{header:?}{:?}", Secret::new("dd-secret-1"));
        assert!(!printed.contains("dd-secret-1"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(header.header_value().expect("value").is_sensitive());
    }

    #[test]
    fn the_probe_gets_only_plain_rows() {
        let conn = integration(
            "headers-plain",
            json!({"url": "https://up/mcp", "headers": [
                {"name": "DD_API_KEY", "secret": true},
                {"name": "X-Grafana-URL", "value": "https://grafana.example.com", "secret": false},
            ]}),
        );
        let plain = plain_headers(&conn).expect("headers");
        assert_eq!(
            plain,
            [StaticHeader {
                name: HeaderName::from_static("x-grafana-url"),
                value: HeaderText::Plain("https://grafana.example.com".to_string()),
            }]
        );
    }
}
