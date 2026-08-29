//! Token-level scan for filesystem/RCE constructs.

use std::sync::LazyLock;

use regex::Regex;

/// A construct that reaches the server's filesystem or shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerousConstruct {
    CopyProgram,
    IntoOutfile,
    LoadData,
    AttachDatabase,
    PgReadFile,
    LoImport,
}

impl DangerousConstruct {
    /// Identifier used in deny reasons and logs (matches TS).
    pub fn as_str(&self) -> &'static str {
        match self {
            DangerousConstruct::CopyProgram => "copy-program",
            DangerousConstruct::IntoOutfile => "into-outfile",
            DangerousConstruct::LoadData => "load-data",
            DangerousConstruct::AttachDatabase => "attach-database",
            DangerousConstruct::PgReadFile => "pg-read-file",
            DangerousConstruct::LoImport => "lo-import",
        }
    }
}

/// Ordered so the reason names the first matching construct, like the TS scan.
static PATTERNS: LazyLock<Vec<(Regex, DangerousConstruct)>> = LazyLock::new(|| {
    let rules: &[(&str, DangerousConstruct)] = &[
        (
            r"(?is)\bCOPY\b.*?\bFROM\s+PROGRAM\b",
            DangerousConstruct::CopyProgram,
        ),
        (
            r"(?is)\bCOPY\b.*?\bTO\s+PROGRAM\b",
            DangerousConstruct::CopyProgram,
        ),
        (r"(?i)\bINTO\s+OUTFILE\b", DangerousConstruct::IntoOutfile),
        (r"(?i)\bLOAD\s+DATA\b", DangerousConstruct::LoadData),
        (
            r#"(?i)\bATTACH\s+(DATABASE\s+)?['"\w]"#,
            DangerousConstruct::AttachDatabase,
        ),
        (r"(?i)\bpg_read_file\b", DangerousConstruct::PgReadFile),
        (r"(?i)\blo_import\b", DangerousConstruct::LoImport),
    ];
    rules
        .iter()
        .map(|(pattern, construct)| {
            (
                Regex::new(pattern).expect("valid dangerous regex"),
                *construct,
            )
        })
        .collect()
});

/// Scan raw SQL for filesystem/RCE constructs the parser may not handle.
///
/// Runs regardless of parse success. Block and `--` comments are stripped
/// first — line-comment stripping keeps the newline, so a comment between
/// `FROM` and `PROGRAM` cannot hide the pair. MySQL's `#` form is *not*
/// stripped here, mirroring `scanDangerous` in the TS engine: text inside a
/// `#` comment still triggers the scan, which can only over-deny, never
/// under-deny.
pub fn scan_dangerous(sql: &str) -> Option<DangerousConstruct> {
    let stripped = crate::keywords::strip_comments(sql, false);
    PATTERNS
        .iter()
        .find(|(re, _)| re.is_match(&stripped))
        .map(|(_, construct)| *construct)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(sql: &str) -> Option<&'static str> {
        scan_dangerous(sql).map(|c| c.as_str())
    }

    #[test]
    fn detects_every_construct() {
        assert_eq!(scan("COPY t FROM PROGRAM 'ls'"), Some("copy-program"));
        assert_eq!(scan("COPY t TO PROGRAM 'ls'"), Some("copy-program"));
        assert_eq!(
            scan("SELECT * FROM t INTO OUTFILE '/tmp/x'"),
            Some("into-outfile")
        );
        assert_eq!(
            scan("LOAD DATA INFILE '/tmp/x' INTO TABLE t"),
            Some("load-data")
        );
        assert_eq!(
            scan("ATTACH DATABASE '/tmp/x' AS aux"),
            Some("attach-database")
        );
        assert_eq!(scan("ATTACH '/tmp/x' AS aux"), Some("attach-database"));
        assert_eq!(
            scan("SELECT pg_read_file('/etc/passwd')"),
            Some("pg-read-file")
        );
        assert_eq!(scan("SELECT lo_import('/etc/passwd')"), Some("lo-import"));
    }

    #[test]
    fn safe_statements_pass() {
        assert_eq!(scan("SELECT 1"), None);
        // Plain COPY to/from a file is not program execution; category gating handles it.
        assert_eq!(scan("COPY t FROM '/tmp/f'"), None);
    }

    #[test]
    fn comments_cannot_hide_a_construct_across_lines() {
        assert_eq!(
            scan("COPY t FROM --note\n PROGRAM 'ls'"),
            Some("copy-program")
        );
        assert_eq!(scan("COPY t /*x*/ FROM PROGRAM 'ls'"), Some("copy-program"));
    }
}
