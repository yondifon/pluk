//! What a driver will actually send to the server.
//!
//! Postgres and SQLite carry parameters on the wire, so the statement they run
//! is the statement they were given. MySQL has no such channel here — values
//! are inlined before the text goes out — so the placeholder form is not what
//! executes. Anything gating or auditing a statement has to work from
//! [`resolve_statement`], never from the raw SQL a caller happened to pass.

use serde_json::Value;

/// Quote a value for inlining into MySQL text.
fn escape_mysql_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            _ => out.push(ch),
        }
    }
    out.push('\'');
    out
}

fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "1".to_string(),
        Value::Bool(false) => "0".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_mysql_string(s),
        Value::Array(_) | Value::Object(_) => escape_mysql_string(&value.to_string()),
    }
}

/// Where the scanner is as it walks a statement looking for placeholders.
#[derive(Clone, Copy, PartialEq)]
enum Scan {
    Sql,
    Quoted(char),
    LineComment,
    BlockComment,
}

/// Inline `params` at the statement's `?` placeholders.
///
/// A `?` only counts as a placeholder outside string literals, quoted
/// identifiers and comments. Substituting inside a literal would wrap the
/// escaped value in the literal's own quotes and hand the rest of it back to
/// the parser as SQL.
pub fn interpolate_mysql(sql: &str, params: &[Value]) -> String {
    let mut out = String::with_capacity(sql.len() + params.len() * 8);
    let mut next = 0usize;
    let mut state = Scan::Sql;
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        match state {
            Scan::Sql => match ch {
                '?' if next < params.len() => {
                    out.push_str(&literal(&params[next]));
                    next += 1;
                    continue;
                }
                '\'' | '"' | '`' => state = Scan::Quoted(ch),
                '-' if chars.peek() == Some(&'-') => state = Scan::LineComment,
                '#' => state = Scan::LineComment,
                '/' if chars.peek() == Some(&'*') => state = Scan::BlockComment,
                _ => {}
            },
            Scan::Quoted(quote) => {
                if ch == '\\' {
                    // The escaped character cannot close the literal.
                    out.push(ch);
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                    continue;
                }
                if ch == quote {
                    // A doubled quote is an escaped quote, not the end.
                    if chars.peek() == Some(&quote) {
                        out.push(ch);
                        out.push(chars.next().expect("peeked"));
                        continue;
                    }
                    state = Scan::Sql;
                }
            }
            Scan::LineComment => {
                if ch == '\n' {
                    state = Scan::Sql;
                }
            }
            Scan::BlockComment => {
                if ch == '*' && chars.peek() == Some(&'/') {
                    out.push(ch);
                    out.push(chars.next().expect("peeked"));
                    state = Scan::Sql;
                    continue;
                }
            }
        }
        out.push(ch);
    }
    out
}

/// The statement and parameters a `driver_type` driver will actually run.
///
/// Gate and log this, not the caller's placeholder form: for MySQL the two
/// differ, and the policy verdict has to describe what reaches the server.
pub fn resolve_statement(driver_type: &str, sql: &str, params: &[Value]) -> (String, Vec<Value>) {
    if driver_type == "mysql" && !params.is_empty() {
        return (interpolate_mysql(sql, params), Vec::new());
    }
    (sql.to_string(), params.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn placeholders_become_quoted_literals() {
        let (sql, params) = resolve_statement(
            "mysql",
            "SELECT * FROM users WHERE name = ? AND age > ? AND active = ?",
            &[json!("o'brien"), json!(30), json!(true)],
        );
        assert_eq!(
            sql,
            "SELECT * FROM users WHERE name = 'o\\'brien' AND age > 30 AND active = 1"
        );
        assert!(
            params.is_empty(),
            "an inlined statement must not be re-interpolated"
        );
    }

    #[test]
    fn a_question_mark_inside_a_literal_is_not_a_placeholder() {
        // Substituting here would close the literal with the value's own
        // quotes and leave the rest as executable SQL.
        let (sql, _) = resolve_statement(
            "mysql",
            "SELECT * FROM t WHERE name = '?'",
            &[json!("x'; DROP TABLE users; -- ")],
        );
        assert_eq!(sql, "SELECT * FROM t WHERE name = '?'");
    }

    #[test]
    fn question_marks_in_comments_and_identifiers_are_left_alone() {
        let (line, _) = resolve_statement("mysql", "SELECT 1 -- why?\n, ?", &[json!(7)]);
        assert_eq!(line, "SELECT 1 -- why?\n, 7");

        let (block, _) = resolve_statement("mysql", "SELECT /* ? */ ?", &[json!(7)]);
        assert_eq!(block, "SELECT /* ? */ 7");

        let (ident, _) = resolve_statement("mysql", "SELECT `we?rd`, ?", &[json!(7)]);
        assert_eq!(ident, "SELECT `we?rd`, 7");
    }

    #[test]
    fn escaped_quotes_do_not_end_a_literal() {
        let (backslash, _) = resolve_statement("mysql", "SELECT 'a\\'?', ?", &[json!(1)]);
        assert_eq!(backslash, "SELECT 'a\\'?', 1");

        let (doubled, _) = resolve_statement("mysql", "SELECT 'a''?', ?", &[json!(1)]);
        assert_eq!(doubled, "SELECT 'a''?', 1");
    }

    #[test]
    fn surplus_placeholders_stay_put() {
        let (sql, _) = resolve_statement("mysql", "SELECT ?, ?", &[json!(1)]);
        assert_eq!(sql, "SELECT 1, ?");
    }

    #[test]
    fn dialects_with_wire_parameters_pass_through_untouched() {
        for driver in ["postgres", "sqlite"] {
            let (sql, params) = resolve_statement(
                driver,
                "SELECT * FROM users WHERE id = $1",
                &[json!("o'brien")],
            );
            assert_eq!(sql, "SELECT * FROM users WHERE id = $1");
            assert_eq!(params, vec![json!("o'brien")]);
        }
    }
}
