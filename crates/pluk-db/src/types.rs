use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub rows: Vec<serde_json::Value>,
    pub fields: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub column: String,
    pub r#type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone)]
pub struct RelationshipInfo {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub constraint_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SchemaSearchResult {
    pub kind: String, // "table" | "column"
    pub table: String,
    pub column: Option<String>,
    pub r#type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TableStats {
    pub table: String,
    pub estimated_rows: Option<i64>,
    pub size_bytes: Option<i64>,
    pub indexes: Vec<IndexInfo>,
}

#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Default)]
pub struct QueryOpts {
    pub timeout_ms: Option<u64>,
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

impl QueryOpts {
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }
    pub fn with_cancel(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }
}
