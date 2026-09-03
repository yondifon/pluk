//! The query policy model: presets, derivation from tool settings,
//! evaluation against classified statements, and post-query row capping.

use serde_json::{Map, Value};

use crate::category::StatementCategory;
use crate::classify::classify;
use crate::dialect::Dialect;

/// Which preset (if any) a policy was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetName {
    ReadOnly,
    ReadWrite,
    Migrations,
    Unrestricted,
    Custom,
}

impl PresetName {
    pub fn as_str(&self) -> &'static str {
        match self {
            PresetName::ReadOnly => "read-only",
            PresetName::ReadWrite => "read-write",
            PresetName::Migrations => "migrations",
            PresetName::Unrestricted => "unrestricted",
            PresetName::Custom => "custom",
        }
    }

    fn from_stored(name: &str) -> Option<Self> {
        match name {
            "read-only" => Some(PresetName::ReadOnly),
            "read-write" => Some(PresetName::ReadWrite),
            "migrations" => Some(PresetName::Migrations),
            "unrestricted" => Some(PresetName::Unrestricted),
            _ => None,
        }
    }
}

/// The full SQL policy for one connection.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryPolicy {
    pub preset: PresetName,
    pub allowed: Vec<StatementCategory>,
    pub block_stacked: bool,
    pub require_where: bool,
    pub allow_filesystem: bool,
    /// Cap on rows returned, applied after execution (`cap_rows`). `None` = uncapped.
    pub max_rows: Option<f64>,
    pub max_estimated_rows: Option<f64>,
    pub max_estimated_cost: Option<f64>,
}

impl QueryPolicy {
    /// The built-in presets. `custom` has none.
    pub fn preset(name: PresetName) -> Option<QueryPolicy> {
        use StatementCategory as C;
        let policy = |allowed: Vec<C>, block_stacked, require_where, allow_filesystem, max_rows| {
            QueryPolicy {
                preset: name,
                allowed,
                block_stacked,
                require_where,
                allow_filesystem,
                max_rows,
                max_estimated_rows: None,
                max_estimated_cost: None,
            }
        };
        match name {
            PresetName::Custom => None,
            PresetName::ReadOnly => Some(policy(
                vec![C::Select, C::Inspect],
                true,
                false,
                false,
                Some(1000.0),
            )),
            PresetName::ReadWrite => Some(policy(
                vec![
                    C::Select,
                    C::Inspect,
                    C::Insert,
                    C::Update,
                    C::Delete,
                    C::Merge,
                    C::Transaction,
                    C::Session,
                ],
                true,
                true,
                false,
                Some(1000.0),
            )),
            PresetName::Migrations => Some(policy(
                vec![
                    C::Select,
                    C::Inspect,
                    C::Insert,
                    C::Update,
                    C::Delete,
                    C::Merge,
                    C::Create,
                    C::Alter,
                    C::Drop,
                    C::Truncate,
                    C::Rename,
                    C::Transaction,
                    C::Session,
                    C::Procedure,
                    C::Maintenance,
                ],
                false,
                true,
                false,
                None,
            )),
            PresetName::Unrestricted => Some(policy(
                crate::category::ALL_CATEGORIES.to_vec(),
                false,
                false,
                true,
                None,
            )),
        }
    }
}

/// Derive the rich policy from the stored settings of the `query` tool.
///
/// The UI stays a three-way mode choice (read-only / mutations / destructive);
/// everything else about the policy is derived from that mode plus the
/// structural guards.
///
/// Mirrors `sqlPolicyFromSettings`: an unrecognized mode falls back to
/// read-only rather than widening access.
pub fn sql_policy_from_settings(settings: &Map<String, Value>) -> QueryPolicy {
    use StatementCategory as C;
    let mode = setting_string(settings, "mode", "read-only");
    let allowed: Vec<C> = match mode.as_str() {
        "mutations" => vec![
            C::Select,
            C::Inspect,
            C::Insert,
            C::Update,
            C::Delete,
            C::Merge,
            C::Transaction,
            C::Session,
        ],
        "destructive" => crate::category::ALL_CATEGORIES.to_vec(),
        _ => vec![C::Select, C::Inspect],
    };
    let max_rows = match read_number(settings, "max_rows") {
        Some(n) if n.is_finite() => {
            if n > 0.0 {
                Some(n)
            } else {
                None // 0 or negative means "no cap"
            }
        }
        // Missing / non-numeric / infinite values fall back to 1000.
        _ => Some(1000.0),
    };
    QueryPolicy {
        preset: PresetName::Custom,
        allowed,
        block_stacked: setting_bool(settings, "block_stacked", true),
        require_where: setting_bool(settings, "require_where", true),
        allow_filesystem: setting_bool(settings, "allow_filesystem", false),
        max_rows,
        max_estimated_rows: None,
        max_estimated_cost: None,
    }
}

/// Default policy per environment: prod/staging start read-only.
pub fn default_policy_for(environment: &str) -> QueryPolicy {
    if environment == "production" || environment == "staging" {
        QueryPolicy::preset(PresetName::ReadOnly).expect("preset exists")
    } else {
        QueryPolicy::preset(PresetName::ReadWrite).expect("preset exists")
    }
}

/// Parse a stored policy JSON string, falling back to the legacy `read_only`
/// flag. Unknown categories are filtered out; unknown presets become `custom`;
/// malformed JSON falls back like an absent blob.
pub fn parse_policy(raw: Option<&str>, legacy_read_only: bool) -> QueryPolicy {
    let parsed = raw
        .filter(|r| !r.is_empty())
        .and_then(|r| serde_json::from_str::<Value>(r).ok())
        .and_then(|value| match value {
            Value::Object(obj) => Some(obj),
            _ => None,
        });
    if let Some(obj) = parsed {
        return stored_policy(&obj);
    }

    if legacy_read_only {
        QueryPolicy::preset(PresetName::ReadOnly).expect("preset exists")
    } else {
        QueryPolicy::preset(PresetName::Unrestricted).expect("preset exists")
    }
}

/// Build the rich policy from an already-parsed stored object, mirroring the
/// TS field handling exactly.
fn stored_policy(obj: &Map<String, Value>) -> QueryPolicy {
    let preset = obj
        .get("preset")
        .and_then(Value::as_str)
        .and_then(PresetName::from_stored)
        .unwrap_or(PresetName::Custom);
    QueryPolicy {
        preset,
        allowed: obj
            .get("allowed")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(StatementCategory::from_id)
                    .collect()
            })
            .unwrap_or_default(),
        block_stacked: obj.get("blockStacked").map(js_truthy).unwrap_or(true),
        require_where: obj.get("requireWhere").map(js_truthy).unwrap_or(false),
        allow_filesystem: obj.get("allowFilesystem").map(js_truthy).unwrap_or(false),
        max_rows: number_or_null(obj.get("maxRows")),
        max_estimated_rows: number_or_null(obj.get("maxEstimatedRows")),
        max_estimated_cost: number_or_null(obj.get("maxEstimatedCost")),
    }
}

/// Only actual JSON numbers count (mirrors `toNumberOrNull`).
fn number_or_null(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// Outcome of evaluating one statement batch.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalResult {
    pub ok: bool,
    pub reason: Option<String>,
    /// Comma-separated category ids, for the audit log.
    pub categories: String,
}

/// Evaluate a SQL string against a policy. Order matters:
/// stacked statements, then dangerous constructs, then unknown/disallowed
/// categories, then UPDATE/DELETE without WHERE.
pub fn evaluate(sql: &str, policy: &QueryPolicy, dialect: Dialect) -> EvalResult {
    let result = classify(sql, dialect);
    let cats = result
        .categories
        .iter()
        .map(|c| c.as_ref().map(StatementCategory::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(",");

    if policy.block_stacked && result.statement_count > 1 {
        return denied(
            format!(
                "Stacked statements blocked ({} statements). Split into separate queries.",
                result.statement_count
            ),
            cats,
        );
    }

    if matches!(dialect, Dialect::MSSQL) && result.has_go_batch {
        return denied(
            "GO batches are blocked. Submit one SQL statement at a time.".to_string(),
            cats,
        );
    }

    if let Some(dangerous) = result.dangerous {
        let mssql_only = matches!(
            dangerous,
            crate::dangerous::DangerousConstruct::XpCmdshell
                | crate::dangerous::DangerousConstruct::BulkInsert
                | crate::dangerous::DangerousConstruct::Openrowset
        );
        if (!mssql_only || matches!(dialect, Dialect::MSSQL))
            && (!policy.allow_filesystem || dangerous.always_blocked())
        {
            return denied(
                format!(
                    "Filesystem/RCE construct '{}' is blocked on this connection.",
                    dangerous.as_str()
                ),
                cats,
            );
        }
    }

    for cat in &result.categories {
        let Some(cat) = cat else {
            return denied(
                "Statement type could not be identified (fail-closed). If this is a valid query, contact the pluk admin."
                    .to_string(),
                cats,
            );
        };
        if !policy.allowed.contains(cat) {
            let allowed_list = policy
                .allowed
                .iter()
                .map(StatementCategory::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            return denied(
                format!(
                    "Statement type '{}' is not allowed on this connection. Allowed: {}.",
                    cat.as_str(),
                    allowed_list
                ),
                cats,
            );
        }
    }

    if policy.require_where && result.has_update_or_delete_without_where {
        return denied(
            "UPDATE or DELETE without a WHERE clause is blocked on this connection (requireWhere)."
                .to_string(),
            cats,
        );
    }

    EvalResult {
        ok: true,
        reason: None,
        categories: cats,
    }
}

fn denied(reason: String, categories: String) -> EvalResult {
    EvalResult {
        ok: false,
        reason: Some(reason),
        categories,
    }
}

/// Rows returned by a capped query.
#[derive(Debug, Clone, PartialEq)]
pub struct CapResult {
    pub rows: Vec<Value>,
    pub truncated: bool,
    pub limit: Option<f64>,
}

/// Apply the row cap after a query has run. Never part of the gate itself.
pub fn cap_rows(rows: Vec<Value>, max_rows: Option<f64>) -> CapResult {
    let Some(max_rows) = max_rows else {
        return CapResult {
            rows,
            truncated: false,
            limit: None,
        };
    };
    let limit = max_rows.trunc().max(0.0);
    if rows.len() as f64 <= limit {
        return CapResult {
            rows,
            truncated: false,
            limit: Some(max_rows),
        };
    }
    let rows = rows.into_iter().take(limit as usize).collect();
    CapResult {
        rows,
        truncated: true,
        limit: Some(max_rows),
    }
}

/// Human-readable summary embedded in MCP tool descriptions.
pub fn policy_description(policy: &QueryPolicy) -> String {
    let caps = policy
        .allowed
        .iter()
        .map(StatementCategory::display_name)
        .collect::<Vec<_>>()
        .join(", ");
    let mut guards: Vec<String> = Vec::new();
    if policy.block_stacked {
        guards.push("no stacked statements".to_string());
    }
    if policy.require_where {
        guards.push("WHERE required on UPDATE/DELETE".to_string());
    }
    if !policy.allow_filesystem {
        guards.push("no filesystem/COPY ops".to_string());
    }
    if let Some(max_rows) = policy.max_rows {
        guards.push(format!("max {max_rows} rows returned"));
    }
    if let Some(max_estimated_rows) = policy.max_estimated_rows {
        guards.push(format!("max {max_estimated_rows} estimated rows"));
    }
    if let Some(max_estimated_cost) = policy.max_estimated_cost {
        guards.push(format!("max {max_estimated_cost} estimated cost"));
    }
    if guards.is_empty() {
        format!("Allowed: {caps}.")
    } else {
        format!("Allowed: {caps}. Guards: {}.", guards.join("; "))
    }
}

/// The cheapest top-level plan cost and row estimate from a PostgreSQL
/// `EXPLAIN (FORMAT JSON)` result. Nulls when values are missing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CostEstimate {
    pub rows: Option<f64>,
    pub cost: Option<f64>,
}

pub fn parse_postgres_cost(plan_json: &Value) -> CostEstimate {
    let plans: Vec<&Value> = match plan_json {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for root in plans {
        if let Some(plan) = root.get("Plan").filter(|p| p.is_object()) {
            return CostEstimate {
                rows: plan.get("Plan Rows").and_then(Value::as_f64),
                cost: plan.get("Total Cost").and_then(Value::as_f64),
            };
        }
    }
    CostEstimate {
        rows: None,
        cost: None,
    }
}


use crate::tool_config::{js_truthy, read_number, setting_bool, setting_string};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::ALL_CATEGORIES;
    use crate::dialect::dialect_for;
    use StatementCategory as C;
    use serde_json::json;

    fn eval(sql: &str, policy: &QueryPolicy, db_type: &str) -> EvalResult {
        evaluate(sql, policy, dialect_for(db_type))
    }


    #[test]
    fn read_only_preset_allows_reads_and_blocks_everything_else() {
        let policy = QueryPolicy::preset(PresetName::ReadOnly).expect("preset exists");
        assert!(eval("SELECT 1", &policy, "postgres").ok);
        assert!(eval("EXPLAIN SELECT 1", &policy, "postgres").ok);
        assert!(eval("PRAGMA table_info(t)", &policy, "sqlite").ok);

        let r = eval("INSERT INTO t VALUES (1)", &policy, "postgres");
        assert!(!r.ok);
        assert!(r.reason.as_deref().is_some_and(|r| r.contains("insert")));
        assert!(!eval("UPDATE t SET x=1 WHERE id=1", &policy, "postgres").ok);
        assert!(!eval("DELETE FROM t WHERE id=1", &policy, "postgres").ok);
        assert!(!eval("DROP TABLE t", &policy, "postgres").ok);
    }

    #[test]
    fn stacked_statements_blocked_then_allowed_by_flag() {
        let mut policy = QueryPolicy::preset(PresetName::ReadOnly).expect("exists");
        policy.block_stacked = true;
        let r = eval("SELECT 1; DROP TABLE t", &policy, "postgres");
        assert!(!r.ok);
        assert!(
            r.reason
                .as_deref()
                .is_some_and(|r| r.to_lowercase().contains("stacked"))
        );

        let mut migrations = QueryPolicy::preset(PresetName::Migrations).expect("exists");
        migrations.block_stacked = false;
        assert!(eval("SELECT 1; SELECT 2", &migrations, "postgres").ok);
    }

    #[test]
    fn require_where_guards_update_and_delete() {
        let policy = QueryPolicy::preset(PresetName::ReadWrite).expect("exists");
        assert!(policy.require_where);

        let r = eval("UPDATE t SET x=1", &policy, "postgres");
        assert!(!r.ok);
        assert!(r.reason.as_deref().is_some_and(|r| r.contains("WHERE")));
        assert!(eval("UPDATE t SET x=1 WHERE id=1", &policy, "postgres").ok);
        assert!(!eval("DELETE FROM t", &policy, "postgres").ok);
    }

    #[test]
    fn filesystem_constructs_respect_allow_filesystem() {
        let unrestricted = QueryPolicy::preset(PresetName::Unrestricted).expect("exists");
        assert!(unrestricted.allow_filesystem);
        assert!(eval("COPY t FROM PROGRAM 'ls'", &unrestricted, "postgres").ok);

        let mut locked = QueryPolicy::preset(PresetName::ReadOnly).expect("exists");
        locked.allow_filesystem = false;
        let r = eval("COPY t FROM PROGRAM 'ls'", &locked, "postgres");
        assert!(!r.ok);
        assert!(
            r.reason
                .as_deref()
                .is_some_and(|r| r.to_lowercase().contains("filesystem"))
        );

        let r = eval("SELECT * FROM t INTO OUTFILE '/tmp/x'", &locked, "mysql");
        assert!(!r.ok);
    }

    #[test]
    fn mssql_server_capabilities_stay_blocked_even_when_filesystem_is_allowed() {
        let policy = QueryPolicy::preset(PresetName::Unrestricted).expect("exists");
        for sql in [
            "EXEC xp_cmdshell 'whoami'",
            "BULK INSERT users FROM '/tmp/users.csv'",
            "SELECT * FROM OPENROWSET(BULK '/tmp/users.csv')",
        ] {
            let result = eval(sql, &policy, "mssql");
            assert!(!result.ok, "{sql} must be blocked");
        }
        assert!(!eval("SELECT 1\nGO\nSELECT 2", &policy, "mssql").ok);
    }

    #[test]
    fn fail_closed_on_unidentifiable_statements_even_when_all_is_allowed() {
        let policy = QueryPolicy::preset(PresetName::Unrestricted).expect("exists");
        let r = eval("XYZZY FROBNICATOR 42", &policy, "postgres");
        assert!(!r.ok);
        assert!(
            r.reason
                .as_deref()
                .is_some_and(|r| r.contains("could not be identified"))
        );
    }

    #[test]
    fn evaluation_order_puts_stacked_first_and_requires_where_last() {
        let mut policy = QueryPolicy::preset(PresetName::ReadOnly).expect("exists");
        policy.block_stacked = true;
        policy.require_where = true;
        // Stacked beats category: first statement's category is reported.
        let r = eval(
            "INSERT INTO t VALUES (1); DROP TABLE t",
            &policy,
            "postgres",
        );
        assert!(r.reason.as_deref().is_some_and(|r| r.contains("Stacked")));
        // Category denial beats require_where.
        let r = eval("UPDATE t SET x=1", &policy, "postgres");
        assert!(
            r.reason
                .as_deref()
                .is_some_and(|r| r.contains("not allowed"))
        );
    }


    #[test]
    fn presets_match_the_ts_definitions() {
        use PresetName::*;
        let read_only = QueryPolicy::preset(ReadOnly).expect("exists");
        assert_eq!(read_only.allowed, vec![C::Select, C::Inspect]);
        assert!(read_only.block_stacked);
        assert!(!read_only.require_where);
        assert!(!read_only.allow_filesystem);
        assert_eq!(read_only.max_rows, Some(1000.0));

        let read_write = QueryPolicy::preset(ReadWrite).expect("exists");
        assert_eq!(
            read_write.allowed,
            vec![
                C::Select,
                C::Inspect,
                C::Insert,
                C::Update,
                C::Delete,
                C::Merge,
                C::Transaction,
                C::Session
            ]
        );
        assert!(read_write.require_where);

        let migrations = QueryPolicy::preset(Migrations).expect("exists");
        assert!(
            !migrations.allowed.contains(&C::Grant),
            "grant stays out of migrations"
        );
        assert!(migrations.allowed.contains(&C::Drop));
        assert!(!migrations.block_stacked);
        assert_eq!(migrations.max_rows, None);

        let unrestricted = QueryPolicy::preset(Unrestricted).expect("exists");
        assert_eq!(unrestricted.allowed, ALL_CATEGORIES.to_vec());
        assert!(unrestricted.allow_filesystem);
        assert_eq!(QueryPolicy::preset(Custom), None);
    }

    #[test]
    fn default_policy_per_environment() {
        for env in ["production", "staging"] {
            assert_eq!(
                default_policy_for(env).preset,
                PresetName::ReadOnly,
                "{env}"
            );
        }
        for env in ["development", "local", ""] {
            assert_eq!(
                default_policy_for(env).preset,
                PresetName::ReadWrite,
                "{env}"
            );
        }
    }


    fn settings_from(json: serde_json::Value) -> Map<String, Value> {
        serde_json::from_value(json).expect("object")
    }

    #[test]
    fn three_modes_map_to_their_category_sets() {
        let read_only = sql_policy_from_settings(&settings_from(json!({"mode": "read-only"})));
        assert_eq!(read_only.allowed, vec![C::Select, C::Inspect]);

        let mutations = sql_policy_from_settings(&settings_from(json!({"mode": "mutations"})));
        assert!(mutations.allowed.contains(&C::Update));
        assert!(!mutations.allowed.contains(&C::Drop));

        let destructive = sql_policy_from_settings(&settings_from(json!({"mode": "destructive"})));
        assert_eq!(destructive.allowed, ALL_CATEGORIES.to_vec());
    }

    #[test]
    fn missing_or_invalid_mode_falls_back_to_read_only() {
        for mode in [json!("bogus"), json!(42), json!(null)] {
            let settings = settings_from(json!({ "mode": mode }));
            assert_eq!(
                sql_policy_from_settings(&settings).allowed,
                vec![C::Select, C::Inspect],
                "{mode}"
            );
        }
        assert_eq!(
            sql_policy_from_settings(&Map::new()).allowed,
            vec![C::Select, C::Inspect]
        );
    }

    #[test]
    fn guard_defaults_are_safe() {
        let policy = sql_policy_from_settings(&Map::new());
        assert!(policy.block_stacked);
        assert!(policy.require_where);
        assert!(!policy.allow_filesystem);
        assert_eq!(policy.preset, PresetName::Custom);
    }

    #[test]
    fn guards_accept_string_booleans_like_the_ui_round_trip() {
        let policy = sql_policy_from_settings(&settings_from(json!({
            "block_stacked": "false", "require_where": "false", "allow_filesystem": "true"
        })));
        assert!(!policy.block_stacked);
        assert!(!policy.require_where);
        assert!(policy.allow_filesystem);
    }

    #[test]
    fn max_rows_follows_the_ts_mapping() {
        // Numbers pass through; positive only.
        assert_eq!(
            sql_policy_from_settings(&settings_from(json!({"max_rows": 500}))).max_rows,
            Some(500.0)
        );
        // Numeric strings count.
        assert_eq!(
            sql_policy_from_settings(&settings_from(json!({"max_rows": "250"}))).max_rows,
            Some(250.0)
        );
        // Zero/negative mean no cap.
        assert_eq!(
            sql_policy_from_settings(&settings_from(json!({"max_rows": 0}))).max_rows,
            None
        );
        assert_eq!(
            sql_policy_from_settings(&settings_from(json!({"max_rows": -5}))).max_rows,
            None
        );
        // Garbage falls back to 1000, never to uncapped.
        assert_eq!(
            sql_policy_from_settings(&settings_from(json!({"max_rows": "abc"}))).max_rows,
            Some(1000.0)
        );
        assert_eq!(sql_policy_from_settings(&Map::new()).max_rows, Some(1000.0));
    }


    #[test]
    fn legacy_read_only_flag_selects_presets() {
        let p = parse_policy(None, true);
        assert_eq!(p.preset, PresetName::ReadOnly);
        assert!(p.allowed.contains(&C::Select));
        assert!(!p.allowed.contains(&C::Insert));

        assert_eq!(parse_policy(None, false).preset, PresetName::Unrestricted);
        assert_eq!(
            parse_policy(Some(""), false).preset,
            PresetName::Unrestricted
        );
    }

    #[test]
    fn valid_policy_json_parses_with_camel_case_fields() {
        let raw = r#"{"preset":"read-write","allowed":["select","insert"],"blockStacked":false,"requireWhere":true,"allowFilesystem":false,"maxRows":500}"#;
        let p = parse_policy(Some(raw), false);
        assert_eq!(p.preset, PresetName::ReadWrite);
        assert_eq!(p.allowed, vec![C::Select, C::Insert]);
        assert!(!p.block_stacked);
        assert!(p.require_where);
        assert_eq!(p.max_rows, Some(500.0));
    }

    #[test]
    fn invalid_json_falls_back_to_legacy_flag() {
        assert_eq!(
            parse_policy(Some("not-json"), true).preset,
            PresetName::ReadOnly
        );
    }

    #[test]
    fn unknown_categories_and_presets_are_filtered() {
        let raw = r#"{"preset":"nope","allowed":["select","foobar"],"blockStacked":true,"requireWhere":false,"allowFilesystem":false,"maxRows":null}"#;
        let p = parse_policy(Some(raw), false);
        assert_eq!(p.preset, PresetName::Custom);
        assert_eq!(p.allowed, vec![C::Select]);
        assert_eq!(p.max_rows, None);
    }

    #[test]
    fn absent_booleans_take_the_ts_defaults() {
        let p = parse_policy(Some("{}"), false);
        assert!(p.block_stacked);
        assert!(!p.require_where);
        assert!(!p.allow_filesystem);
        assert_eq!(p.max_rows, None);
        assert_eq!(p.max_estimated_rows, None);
        assert_eq!(p.max_estimated_cost, None);
    }


    fn rows(n: usize) -> Vec<Value> {
        (0..n).map(|i| json!({"id": i})).collect()
    }

    #[test]
    fn cap_rows_never_caps_when_unset_or_under_limit() {
        let r = cap_rows(rows(50), None);
        assert_eq!(r.rows.len(), 50);
        assert!(!r.truncated);
        assert_eq!(r.limit, None);

        let r = cap_rows(rows(50), Some(100.0));
        assert_eq!(r.rows.len(), 50);
        assert!(!r.truncated);
        assert_eq!(r.limit, Some(100.0));
    }

    #[test]
    fn cap_rows_truncates_and_flags() {
        let r = cap_rows(rows(50), Some(10.0));
        assert_eq!(r.rows.len(), 10);
        assert!(r.truncated);
        assert_eq!(r.limit, Some(10.0));
    }


    #[test]
    fn parses_plan_cost_and_row_estimate() {
        let plan = json!([{ "Plan": { "Plan Rows": 42, "Total Cost": 123.45 } }]);
        let estimate = parse_postgres_cost(&plan);
        assert_eq!(estimate.rows, Some(42.0));
        assert_eq!(estimate.cost, Some(123.45));
    }

    #[test]
    fn missing_plan_yields_nulls() {
        let estimate = parse_postgres_cost(&json!([{}]));
        assert_eq!(
            estimate,
            CostEstimate {
                rows: None,
                cost: None
            }
        );
        assert_eq!(
            parse_postgres_cost(&Value::Null),
            CostEstimate {
                rows: None,
                cost: None
            }
        );
    }


    #[test]
    fn description_matches_the_ts_format() {
        let read_only = QueryPolicy::preset(PresetName::ReadOnly).expect("exists");
        assert_eq!(
            policy_description(&read_only),
            "Allowed: SELECT, DESCRIBE/EXPLAIN/SHOW. Guards: no stacked statements; no filesystem/COPY ops; max 1000 rows returned."
        );
        let unrestricted = QueryPolicy::preset(PresetName::Unrestricted).expect("exists");
        assert_eq!(
            policy_description(&unrestricted),
            "Allowed: SELECT, DESCRIBE/EXPLAIN/SHOW, INSERT, UPDATE, DELETE, MERGE/REPLACE, CREATE, ALTER, DROP, TRUNCATE, RENAME, BEGIN/COMMIT/ROLLBACK, SET/USE, CALL/DO, VACUUM/ANALYZE, GRANT/REVOKE."
        );
    }


    #[test]
    fn destructive_mode_still_denies_unidentified_statements_and_load_data() {
        let settings = settings_from(json!({"mode": "destructive", "allow_filesystem": true}));
        let policy = sql_policy_from_settings(&settings);
        // LOAD DATA parses but maps to no safe category — denied like the TS server,
        // whose AST type `load_data` is unmapped too.
        let r = eval("LOAD DATA INFILE '/tmp/x' INTO TABLE t", &policy, "mysql");
        assert!(!r.ok);
    }
}
