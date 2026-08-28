//! Row capping and masking seams.
//!
//! Row capping happens after a query returns. Masked columns are applied
//! after capping and before both the response text and the log snapshot, so
//! the audit log never contains unmasked values. This module provides the
//! hooks; R09 wires the SQL tools through them.

use serde_json::Value;

/// Cap rows to `limit`. Returns (capped_rows, was_capped, total).
pub fn cap_rows(rows: Vec<Value>, limit: usize) -> (Vec<Value>, bool, usize) {
    let total = rows.len();
    if rows.len() > limit {
        (rows.into_iter().take(limit).collect(), true, total)
    } else {
        (rows, false, total)
    }
}

/// Mask listed columns in rows. Replaces matching column values with "***".
/// `masked_columns` are column names to mask. Order: after capping, before
/// response/log — caller must ensure log snapshot also uses masked rows.
pub fn mask_columns(rows: &mut [Value], masked_columns: &[String]) {
    if masked_columns.is_empty() {
        return;
    }
    for row in rows.iter_mut() {
        if let Value::Object(map) = row {
            for col in masked_columns {
                if map.contains_key(col) {
                    map.insert(col.clone(), Value::String("***".to_string()));
                }
            }
        }
    }
}

/// Combined hook for R09: cap then mask. Returns (rows, was_capped, total).
pub fn cap_and_mask(rows: Vec<Value>, cap: usize, masked: &[String]) -> (Vec<Value>, bool, usize) {
    let (mut capped, was_capped, total) = cap_rows(rows, cap);
    // Need mut rows — cap_rows already consumed
    // Re-apply masking on capped set
    // We already have capped; mask in place
    // Do it directly:
    mask_columns(&mut capped, masked);
    (capped, was_capped, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cap_rows_truncates() {
        let rows = vec![json!({"a":1}), json!({"a":2}), json!({"a":3})];
        let (capped, was_capped, total) = cap_rows(rows, 2);
        assert_eq!(capped.len(), 2);
        assert!(was_capped);
        assert_eq!(total, 3);
    }
    #[test]
    fn cap_rows_no_truncation() {
        let rows = vec![json!({"a":1})];
        let (capped, was_capped, total) = cap_rows(rows, 10);
        assert_eq!(capped.len(), 1);
        assert!(!was_capped);
        assert_eq!(total, 1);
    }
    #[test]
    fn mask_replaces_values() {
        let mut rows = vec![json!({"secret":"hunter2","name":"alice"})];
        mask_columns(&mut rows, &["secret".to_string()]);
        assert_eq!(rows[0]["secret"], "***");
        assert_eq!(rows[0]["name"], "alice");
    }
    #[test]
    fn audit_log_never_sees_unmasked() {
        // Simulate R09 order: cap -> mask -> log snapshot
        let rows = vec![
            json!({"ssn":"123","id":1}),
            json!({"ssn":"456","id":2}),
            json!({"ssn":"789","id":3}),
        ];
        let (masked, _, _) = cap_and_mask(rows, 2, &["ssn".to_string()]);
        assert_eq!(masked.len(), 2);
        for r in &masked {
            assert_eq!(r["ssn"], "***");
        }
        // Ensure original values not present in serialized log snapshot
        let serialized = serde_json::to_string(&masked).unwrap();
        assert!(!serialized.contains("123"));
    }
}
