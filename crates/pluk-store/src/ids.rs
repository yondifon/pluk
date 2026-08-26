//! Identifier and token generation, matching the existing writers exactly.
//!
//! Both current codebases mint ids and tokens from UUIDs:
//!
//! - id: a lowercase UUID with dashes stripped, truncated to 16 hex chars.
//! - token: `pluk_` followed by a dash-stripped lowercase UUID (32 hex chars).
//!
//! The TypeScript server draws raw random bytes instead (`randomBytes(8)` /
//! `randomBytes(12)`), which yields the same shapes: `[0-9a-f]{16}` ids and
//! `pluk_[0-9a-f]{24..32}` tokens. Nothing downstream validates length beyond
//! the `pluk_` prefix, so the UUID-derived form is compatible with every row
//! already in the database.

use uuid::Uuid;

/// A 16-char lowercase hex integration/group id.
pub fn new_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// An MCP endpoint token: `pluk_` + 32 lowercase hex chars.
pub fn new_token() -> String {
    format!("pluk_{}", Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sixteen_lowercase_hex_chars() {
        for _ in 0..100 {
            let id = new_id();
            assert_eq!(id.len(), 16);
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn tokens_are_pluk_prefixed_lowercase_hex() {
        for _ in 0..100 {
            let body = new_token()
                .strip_prefix("pluk_")
                .expect("pluk_ prefix")
                .to_owned();
            assert_eq!(body.len(), 32);
            assert!(
                body.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
        }
    }
}
