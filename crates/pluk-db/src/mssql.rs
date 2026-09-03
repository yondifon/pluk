#[cfg(feature = "mssql")]
pub mod live {
    use async_trait::async_trait;
    use futures_util::TryStreamExt;
    use serde_json::Value;
    use tiberius::{
        AuthMethod, Client, ColumnData, Config, EncryptionLevel, Query, QueryItem, Row,
    };
    use tokio::net::TcpStream;
    use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

    use crate::driver::Driver;
    use crate::error::DriverError;
    use crate::types::*;

    type SqlClient = Client<Compat<TcpStream>>;

    fn conn_error(host: &str, port: u16, error: impl std::fmt::Display) -> DriverError {
        let message = error.to_string();
        let lower = message.to_lowercase();
        if lower.contains("connection refused") || lower.contains("econnrefused") {
            DriverError::Connection(format!(
                "Connection refused to {host}:{port}. Check host, port, firewall, and SSH tunnel config. ({message})"
            ))
        } else if lower.contains("no such host")
            || lower.contains("name or service not known")
            || lower.contains("enotfound")
        {
            DriverError::Connection(format!(
                "Host not found {host}. Check the host name. ({message})"
            ))
        } else if lower.contains("timed out") || lower.contains("timeout") {
            DriverError::Connection(format!(
                "Timed out connecting to {host}:{port}. Check host, port, SSH tunnel, and firewall/VPC rules. ({message})"
            ))
        } else if lower.contains("login failed")
            || lower.contains("password")
            || lower.contains("authentication")
        {
            DriverError::Connection(format!(
                "Database authentication failed for {host}:{port}. Check username and password. ({message})"
            ))
        } else if lower.contains("certificate") || lower.contains("tls") {
            DriverError::Connection(format!(
                "TLS error connecting to {host}:{port}. Check Encrypt and Trust server certificate settings. ({message})"
            ))
        } else {
            DriverError::Connection(format!("connection failed to {host}:{port}: {message}"))
        }
    }

    fn query_error(error: tiberius::error::Error) -> DriverError {
        DriverError::Query(error.to_string())
    }

    fn cell_to_json(cell: &ColumnData<'static>) -> Value {
        match cell {
            ColumnData::U8(value) => value.map_or(Value::Null, |value| serde_json::json!(value)),
            ColumnData::I16(value) => value.map_or(Value::Null, |value| serde_json::json!(value)),
            ColumnData::I32(value) => value.map_or(Value::Null, |value| serde_json::json!(value)),
            ColumnData::I64(value) => value.map_or(Value::Null, |value| serde_json::json!(value)),
            ColumnData::F32(value) => value
                .and_then(|value| serde_json::Number::from_f64(value as f64))
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ColumnData::F64(value) => value
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ColumnData::Bit(value) => value.map_or(Value::Null, |value| serde_json::json!(value)),
            ColumnData::String(value) => value
                .as_deref()
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            ColumnData::Guid(value) => value
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            ColumnData::Binary(value) => match value.as_deref() {
                None => Value::Null,
                Some(bytes) => match std::str::from_utf8(bytes) {
                    Ok(text) => Value::String(text.to_string()),
                    Err(_) => {
                        Value::Array(bytes.iter().map(|byte| serde_json::json!(*byte)).collect())
                    }
                },
            },
            ColumnData::Numeric(value) => value
                .as_ref()
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            ColumnData::Xml(value) => value
                .as_deref()
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            ColumnData::DateTime(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
            ColumnData::SmallDateTime(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
            ColumnData::Time(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
            ColumnData::Date(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
            ColumnData::DateTime2(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
            ColumnData::DateTimeOffset(value) => value
                .as_ref()
                .map(|value| Value::String(format!("{value:?}")))
                .unwrap_or(Value::Null),
        }
    }

    fn row_to_json(row: &Row) -> Value {
        let mut map = serde_json::Map::new();
        for (column, cell) in row.cells() {
            map.insert(column.name().to_string(), cell_to_json(cell));
        }
        Value::Object(map)
    }

    async fn run_simple_query(client: &mut SqlClient, sql: &str) -> Result<(), DriverError> {
        let mut stream = client.simple_query(sql).await.map_err(query_error)?;
        while stream.try_next().await.map_err(query_error)?.is_some() {}
        Ok(())
    }

    fn bind_value<'a>(query: &mut Query<'a>, value: &Value) {
        match value {
            Value::Null => query.bind(Option::<String>::None),
            Value::Bool(value) => query.bind(*value),
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    query.bind(value);
                } else if let Some(value) = value.as_u64() {
                    if let Ok(value) = i64::try_from(value) {
                        query.bind(value);
                    } else {
                        query.bind(value.to_string());
                    }
                } else if let Some(value) = value.as_f64() {
                    query.bind(value);
                } else {
                    query.bind(value.to_string());
                }
            }
            Value::String(value) => query.bind(value.clone()),
            Value::Array(_) | Value::Object(_) => query.bind(value.to_string()),
        }
    }

    async fn run_query(
        client: &mut SqlClient,
        sql: &str,
        params: &[Value],
    ) -> Result<QueryResult, DriverError> {
        let mut query = Query::new(sql.to_owned());
        for value in params {
            bind_value(&mut query, value);
        }
        let mut stream = query.query(client).await.map_err(query_error)?;
        let mut fields = None;
        let mut rows = Vec::new();
        while let Some(item) = stream.try_next().await.map_err(query_error)? {
            match item {
                QueryItem::Metadata(metadata) if fields.is_none() => {
                    fields = Some(
                        metadata
                            .columns()
                            .iter()
                            .map(|column| column.name().to_string())
                            .collect(),
                    );
                }
                QueryItem::Metadata(_) => {}
                QueryItem::Row(row) => rows.push(row_to_json(&row)),
            }
        }
        Ok(QueryResult { rows, fields })
    }

    fn string(row: &Value, key: &str) -> String {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn optional_string(row: &Value, key: &str) -> Option<String> {
        row.get(key).and_then(Value::as_str).map(str::to_string)
    }

    fn integer(row: &Value, key: &str) -> Option<i64> {
        row.get(key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
    }

    fn quote_identifier(identifier: &str) -> String {
        format!("[{}]", identifier.replace(']', "]]"))
    }

    pub struct MsSqlDriver {
        host: String,
        port: u16,
        user: Option<String>,
        password: Option<String>,
        database: Option<String>,
        encrypt: bool,
        trust_cert: bool,
    }

    impl MsSqlDriver {
        pub fn new(
            host: String,
            port: u16,
            user: Option<String>,
            password: Option<String>,
            database: Option<String>,
            encrypt: bool,
            trust_cert: bool,
        ) -> Self {
            Self {
                host,
                port,
                user,
                password,
                database,
                encrypt,
                trust_cert,
            }
        }

        async fn connect(&self, read_only: bool) -> Result<SqlClient, DriverError> {
            let mut config = Config::new();
            config.host(&self.host);
            config.port(self.port);
            config.application_name("pluk");
            config.encryption(if self.encrypt {
                EncryptionLevel::Required
            } else {
                EncryptionLevel::NotSupported
            });
            config.readonly(read_only);
            if self.trust_cert {
                config.trust_cert();
            }
            if let Some(database) = &self.database {
                config.database(database);
            }
            if let Some(user) = &self.user {
                config.authentication(AuthMethod::sql_server(
                    user,
                    self.password.as_deref().unwrap_or_default(),
                ));
            }

            let tcp = TcpStream::connect(config.get_addr())
                .await
                .map_err(|error| conn_error(&self.host, self.port, error))?;
            tcp.set_nodelay(true)
                .map_err(|error| conn_error(&self.host, self.port, error))?;
            Client::connect(config, tcp.compat_write())
                .await
                .map_err(|error| conn_error(&self.host, self.port, error))
        }

        async fn run(
            &self,
            sql: &str,
            params: &[Value],
            opts: Option<QueryOpts>,
            read_only: bool,
        ) -> Result<QueryResult, DriverError> {
            let sql = sql.to_string();
            let params = params.to_vec();
            let this = Self {
                host: self.host.clone(),
                port: self.port,
                user: self.user.clone(),
                password: self.password.clone(),
                database: self.database.clone(),
                encrypt: self.encrypt,
                trust_cert: self.trust_cert,
            };
            let fut = async move {
                let mut client = this.connect(read_only).await?;
                let result = run_query(&mut client, &sql, &params).await;
                let _ = client.close().await;
                result
            };
            crate::driver::with_opts(opts, fut).await
        }

        async fn query_rows(
            &self,
            sql: &str,
            params: &[Value],
        ) -> Result<QueryResult, DriverError> {
            self.run(sql, params, None, false).await
        }
    }

    #[async_trait]
    impl Driver for MsSqlDriver {
        async fn query(
            &self,
            sql: &str,
            params: &[Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            crate::sql_log::record_executed_sql(sql, None, None);
            let result = self.run(sql, params, opts, false).await;
            if let Ok(result) = &result {
                crate::sql_log::record_executed_sql(sql, Some(result.rows.len() as i64), None);
            }
            result
        }

        async fn query_read_only(
            &self,
            sql: &str,
            params: &[Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            self.run(sql, params, opts, true).await
        }

        async fn explain(&self, sql: &str, params: &[Value]) -> Result<QueryResult, DriverError> {
            let mut client = self.connect(true).await?;
            run_simple_query(&mut client, "SET SHOWPLAN_TEXT ON").await?;
            let result = run_query(&mut client, sql, params).await;
            run_simple_query(&mut client, "SET SHOWPLAN_TEXT OFF").await?;
            let result = result?;
            crate::sql_log::record_executed_sql(sql, Some(result.rows.len() as i64), None);
            Ok(result)
        }

        async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DriverError> {
            let result = self
                .query_rows(
                    "SELECT TABLE_NAME AS table_name FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE = 'BASE TABLE' AND (@P1 IS NULL OR TABLE_SCHEMA = @P1) ORDER BY TABLE_SCHEMA, TABLE_NAME",
                    &[serde_json::json!(schema)],
                )
                .await?;
            Ok(result
                .rows
                .iter()
                .map(|row| string(row, "table_name"))
                .collect())
        }

        async fn describe_table(
            &self,
            table: &str,
            schema: Option<&str>,
        ) -> Result<Vec<ColumnInfo>, DriverError> {
            let result = self
                .query_rows(
                    "SELECT COLUMN_NAME AS column_name, DATA_TYPE AS data_type, IS_NULLABLE AS is_nullable FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = @P1 AND (@P2 IS NULL OR TABLE_SCHEMA = @P2) ORDER BY ORDINAL_POSITION",
                    &[serde_json::json!(table), serde_json::json!(schema)],
                )
                .await?;
            Ok(result
                .rows
                .iter()
                .map(|row| ColumnInfo {
                    column: string(row, "column_name"),
                    r#type: string(row, "data_type"),
                    nullable: string(row, "is_nullable") == "YES",
                })
                .collect())
        }

        async fn sample_table(
            &self,
            table: &str,
            limit: i64,
            schema: Option<&str>,
        ) -> Result<QueryResult, DriverError> {
            let table = quote_identifier(table);
            let table = match schema {
                Some(schema) => format!("{}.{}", quote_identifier(schema), table),
                None => table,
            };
            let sql = format!("SELECT TOP (@P1) * FROM {table}");
            self.query_rows(&sql, &[serde_json::json!(limit.max(0))])
                .await
        }

        async fn list_relationships(
            &self,
            table: Option<&str>,
            schema: Option<&str>,
        ) -> Result<Vec<RelationshipInfo>, DriverError> {
            let result = self
                .query_rows(
                    "SELECT ps.name AS from_schema, pt.name AS from_table, pc.name AS from_column, rs.name AS to_schema, rt.name AS to_table, rc.name AS to_column, fk.name AS constraint_name FROM sys.foreign_keys fk JOIN sys.foreign_key_columns fkc ON fk.object_id = fkc.constraint_object_id JOIN sys.tables pt ON pt.object_id = fkc.parent_object_id JOIN sys.schemas ps ON ps.schema_id = pt.schema_id JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id JOIN sys.tables rt ON rt.object_id = fkc.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id WHERE (@P1 IS NULL OR pt.name = @P1) AND (@P2 IS NULL OR ps.name = @P2) ORDER BY pt.name, fk.name, fkc.constraint_column_id",
                    &[serde_json::json!(table), serde_json::json!(schema)],
                )
                .await?;
            Ok(result
                .rows
                .iter()
                .map(|row| RelationshipInfo {
                    from_table: string(row, "from_table"),
                    from_column: string(row, "from_column"),
                    to_table: string(row, "to_table"),
                    to_column: string(row, "to_column"),
                    constraint_name: optional_string(row, "constraint_name"),
                })
                .collect())
        }

        async fn search_schema(
            &self,
            term: &str,
            schema: Option<&str>,
        ) -> Result<Vec<SchemaSearchResult>, DriverError> {
            let pattern = format!(
                "%{}%",
                term.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let result = self
                .query_rows(
                    "SELECT 'table' AS kind, TABLE_NAME AS table_name, CAST(NULL AS nvarchar(128)) AS column_name, CAST(NULL AS nvarchar(128)) AS data_type FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE = 'BASE TABLE' AND TABLE_NAME LIKE @P1 ESCAPE '\\' AND (@P2 IS NULL OR TABLE_SCHEMA = @P2) UNION ALL SELECT 'column', TABLE_NAME, COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE (TABLE_NAME LIKE @P1 ESCAPE '\\' OR COLUMN_NAME LIKE @P1 ESCAPE '\\') AND (@P2 IS NULL OR TABLE_SCHEMA = @P2) ORDER BY table_name, kind, column_name",
                    &[serde_json::json!(pattern), serde_json::json!(schema)],
                )
                .await?;
            Ok(result
                .rows
                .iter()
                .map(|row| SchemaSearchResult {
                    kind: string(row, "kind"),
                    table: string(row, "table_name"),
                    column: optional_string(row, "column_name"),
                    r#type: optional_string(row, "data_type"),
                })
                .collect())
        }

        async fn table_stats(
            &self,
            table: &str,
            schema: Option<&str>,
        ) -> Result<TableStats, DriverError> {
            let schema = schema.unwrap_or("dbo");
            let stats = self
                .query_rows(
                    "SELECT SUM(p.rows) AS estimated_rows, SUM(a.total_pages) * 8192 AS size_bytes FROM sys.tables t JOIN sys.schemas s ON s.schema_id = t.schema_id JOIN sys.indexes i ON i.object_id = t.object_id JOIN sys.partitions p ON p.object_id = i.object_id AND p.index_id = i.index_id JOIN sys.allocation_units a ON a.container_id = p.partition_id WHERE t.name = @P1 AND s.name = @P2",
                    &[serde_json::json!(table), serde_json::json!(schema)],
                )
                .await?;
            let (estimated_rows, size_bytes) = stats.rows.first().map_or((None, None), |row| {
                (integer(row, "estimated_rows"), integer(row, "size_bytes"))
            });
            let index_rows = self
                .query_rows(
                    "SELECT i.name AS index_name, c.name AS column_name, i.is_unique AS is_unique FROM sys.indexes i JOIN sys.tables t ON t.object_id = i.object_id JOIN sys.schemas s ON s.schema_id = t.schema_id JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id WHERE t.name = @P1 AND s.name = @P2 AND i.name IS NOT NULL ORDER BY i.name, ic.key_ordinal",
                    &[serde_json::json!(table), serde_json::json!(schema)],
                )
                .await?;
            let mut indexes = std::collections::BTreeMap::<String, (Vec<String>, bool)>::new();
            for row in index_rows.rows {
                let name = string(&row, "index_name");
                let entry = indexes.entry(name).or_insert_with(|| {
                    (
                        Vec::new(),
                        row.get("is_unique")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                });
                entry.0.push(string(&row, "column_name"));
            }
            Ok(TableStats {
                table: table.to_string(),
                estimated_rows,
                size_bytes,
                indexes: indexes
                    .into_iter()
                    .map(|(name, (columns, unique))| IndexInfo {
                        name,
                        columns,
                        unique,
                    })
                    .collect(),
            })
        }

        async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
            let result = self
                .query_rows("SELECT name FROM sys.schemas ORDER BY name", &[])
                .await?;
            Ok(result.rows.iter().map(|row| string(row, "name")).collect())
        }

        async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
            let result = self
                .query_rows(
                    "SELECT name FROM sys.databases WHERE state = 0 ORDER BY name",
                    &[],
                )
                .await?;
            Ok(result.rows.iter().map(|row| string(row, "name")).collect())
        }

        async fn get_full_schema(&self, schema: Option<&str>) -> Result<String, DriverError> {
            let tables = self.list_tables(schema).await?;
            let mut lines = Vec::new();
            for table in tables {
                let columns = self.describe_table(&table, schema).await?;
                lines.push(format!("TABLE {table} ("));
                for column in columns {
                    let nullability = if column.nullable { "NULL" } else { "NOT NULL" };
                    lines.push(format!(
                        "  {} {} {}",
                        column.column, column.r#type, nullability
                    ));
                }
                lines.push(")".to_string());
                for relation in self.list_relationships(Some(&table), schema).await? {
                    lines.push(format!(
                        "FK {}.{} -> {}.{}",
                        relation.from_table,
                        relation.from_column,
                        relation.to_table,
                        relation.to_column
                    ));
                }
                lines.push(String::new());
            }
            Ok(lines.join("\n").trim().to_string())
        }

        async fn test_connection(&self) -> Result<(), DriverError> {
            self.query_rows("SELECT 1", &[]).await.map(|_| ())
        }

        async fn close(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }
}
