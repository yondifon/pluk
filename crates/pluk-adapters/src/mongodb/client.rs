//! Connection config, the lazily-opened driver client, and the operations the
//! tools call. TLS, replica sets and SRV lookups all come from the connection
//! string, so there is nothing to configure beside it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::TryStreamExt;
use serde_json::{Value, json};

use ::bson::{Bson, Document, doc};
use ::mongodb::options::ClientOptions;
use ::mongodb::{Client, Database};

use crate::error::AdapterError;

/// The most documents any one call returns.
pub const DOCUMENT_CAP: i64 = 1000;

/// How deep the sampled field shape descends into embedded documents.
const SHAPE_DEPTH: usize = 3;

#[derive(Debug, Clone)]
pub struct MongoConfig {
    pub uri: String,
    /// Database used when a call names none.
    pub database: Option<String>,
}

fn trimmed(config: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn mongo_config_from(conn: &pluk_store::Integration) -> Result<MongoConfig, AdapterError> {
    let Some(uri) = trimmed(&conn.config, "uri") else {
        return Err(AdapterError::new(
            "MongoDB connection string is missing. Set it in the integration config.",
        ));
    };
    if !uri.starts_with("mongodb://") && !uri.starts_with("mongodb+srv://") {
        return Err(AdapterError::new(
            "MongoDB connection string must start with mongodb:// or mongodb+srv://.",
        ));
    }
    Ok(MongoConfig {
        uri,
        database: trimmed(&conn.config, "database"),
    })
}

fn failed(operation: &str, error: ::mongodb::error::Error) -> AdapterError {
    AdapterError::new(format!("MongoDB {operation} failed: {error}"))
}

/// Extended JSON in, BSON out — so `{"_id": {"$oid": "…"}}` reaches the server
/// as an ObjectId rather than as a subdocument that matches nothing.
pub fn to_document(value: &Value, what: &str) -> Result<Document, AdapterError> {
    let bson = Bson::try_from(value.clone())
        .map_err(|e| AdapterError::new(format!("`{what}` is not valid JSON for MongoDB: {e}")))?;
    match bson {
        Bson::Document(document) => Ok(document),
        _ => Err(AdapterError::new(format!("`{what}` must be a JSON object."))),
    }
}

/// A pipeline is an array of stage documents.
pub fn to_pipeline(value: &Value) -> Result<Vec<Document>, AdapterError> {
    let Value::Array(stages) = value else {
        return Err(AdapterError::new(
            "`pipeline` must be a JSON array of aggregation stages.",
        ));
    };
    stages
        .iter()
        .map(|stage| to_document(stage, "pipeline stage"))
        .collect()
}

fn to_json(document: Document) -> Value {
    Bson::Document(document).into_relaxed_extjson()
}

/// Clamp a requested document count into `1..=DOCUMENT_CAP`.
pub fn capped(requested: Option<i64>, fallback: i64) -> i64 {
    requested.unwrap_or(fallback).clamp(1, DOCUMENT_CAP)
}

#[derive(Clone)]
pub struct MongoAccessor {
    config: MongoConfig,
    cell: Arc<tokio::sync::OnceCell<Client>>,
}

impl MongoAccessor {
    pub fn new(config: MongoConfig) -> Self {
        MongoAccessor {
            config,
            cell: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    async fn client(&self) -> Result<&Client, AdapterError> {
        self.cell
            .get_or_try_init(|| async {
                let options = ClientOptions::parse(&self.config.uri).await.map_err(|e| {
                    AdapterError::new(format!("MongoDB connection string is not valid: {e}"))
                })?;
                Client::with_options(options)
                    .map_err(|e| AdapterError::new(format!("MongoDB client error: {e}")))
            })
            .await
    }

    /// Which database a call runs against: the argument, then the
    /// integration's default, then the one the connection string names.
    async fn database(&self, requested: Option<&str>) -> Result<Database, AdapterError> {
        let client = self.client().await?;
        let named = requested
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.config.database.clone());
        if let Some(name) = named {
            return Ok(client.database(&name));
        }
        client.default_database().ok_or_else(|| {
            AdapterError::new(
                "No database selected. Pass `database`, or set a default on the integration.",
            )
        })
    }

    pub async fn ping(&self) -> Result<(), AdapterError> {
        let client = self.client().await?;
        let db = match self.database(None).await {
            Ok(db) => db,
            Err(_) => client.database("admin"),
        };
        db.run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| failed("ping", e))?;
        Ok(())
    }

    pub async fn list_databases(&self) -> Result<Value, AdapterError> {
        let client = self.client().await?;
        let specs = client
            .list_databases()
            .await
            .map_err(|e| failed("list databases", e))?;
        let databases: Vec<Value> = specs
            .into_iter()
            .map(|spec| {
                json!({
                    "name": spec.name,
                    "size_on_disk": spec.size_on_disk,
                    "empty": spec.empty,
                })
            })
            .collect();
        Ok(json!({ "databases": databases }))
    }

    pub async fn list_collections(&self, database: Option<&str>) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let names = db
            .list_collection_names()
            .await
            .map_err(|e| failed("list collections", e))?;
        Ok(json!({ "database": db.name(), "collections": names }))
    }

    pub async fn describe_collection(
        &self,
        database: Option<&str>,
        collection: &str,
        sample: i64,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let coll = db.collection::<Document>(collection);

        let mut indexes = Vec::new();
        let mut cursor = coll
            .list_indexes()
            .await
            .map_err(|e| failed("list indexes", e))?;
        while let Some(model) = cursor
            .try_next()
            .await
            .map_err(|e| failed("list indexes", e))?
        {
            let options = model.options.as_ref();
            indexes.push(json!({
                "name": options.and_then(|o| o.name.clone()),
                "key": to_json(model.keys),
                "unique": options.and_then(|o| o.unique).unwrap_or(false),
            }));
        }

        let mut documents = Vec::new();
        let mut cursor = coll
            .find(Document::new())
            .limit(sample)
            .await
            .map_err(|e| failed("sample", e))?;
        while let Some(document) = cursor.try_next().await.map_err(|e| failed("sample", e))? {
            documents.push(document);
        }

        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "indexes": indexes,
            "sampled": documents.len(),
            "fields": sampled_shape(&documents),
        }))
    }

    pub async fn find(
        &self,
        database: Option<&str>,
        collection: &str,
        filter: Document,
        projection: Option<Document>,
        sort: Option<Document>,
        limit: i64,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let coll = db.collection::<Document>(collection);
        // One past the limit tells the agent its page was cut short.
        let mut find = coll.find(filter).limit(limit + 1);
        if let Some(projection) = projection {
            find = find.projection(projection);
        }
        if let Some(sort) = sort {
            find = find.sort(sort);
        }
        let cursor = find.await.map_err(|e| failed("find", e))?;
        let (documents, truncated) = drain(cursor, limit, "find").await?;
        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "documents": documents,
            "returned": documents.len(),
            "truncated": truncated,
        }))
    }

    pub async fn count(
        &self,
        database: Option<&str>,
        collection: &str,
        filter: Document,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let count = db
            .collection::<Document>(collection)
            .count_documents(filter)
            .await
            .map_err(|e| failed("count", e))?;
        Ok(json!({ "database": db.name(), "collection": collection, "count": count }))
    }

    pub async fn aggregate(
        &self,
        database: Option<&str>,
        collection: &str,
        pipeline: Vec<Document>,
        limit: i64,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let cursor = db
            .collection::<Document>(collection)
            .aggregate(pipeline)
            .await
            .map_err(|e| failed("aggregate", e))?;
        let (documents, truncated) = drain(cursor, limit, "aggregate").await?;
        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "documents": documents,
            "returned": documents.len(),
            "truncated": truncated,
        }))
    }

    pub async fn insert_one(
        &self,
        database: Option<&str>,
        collection: &str,
        document: Document,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let result = db
            .collection::<Document>(collection)
            .insert_one(document)
            .await
            .map_err(|e| failed("insert", e))?;
        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "inserted_id": result.inserted_id.into_relaxed_extjson(),
        }))
    }

    pub async fn update_many(
        &self,
        database: Option<&str>,
        collection: &str,
        filter: Document,
        update: Document,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let result = db
            .collection::<Document>(collection)
            .update_many(filter, update)
            .await
            .map_err(|e| failed("update", e))?;
        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "matched": result.matched_count,
            "modified": result.modified_count,
        }))
    }

    pub async fn delete_many(
        &self,
        database: Option<&str>,
        collection: &str,
        filter: Document,
    ) -> Result<Value, AdapterError> {
        let db = self.database(database).await?;
        let result = db
            .collection::<Document>(collection)
            .delete_many(filter)
            .await
            .map_err(|e| failed("delete", e))?;
        Ok(json!({
            "database": db.name(),
            "collection": collection,
            "deleted": result.deleted_count,
        }))
    }
}

/// Read a cursor down to `limit` documents, reporting whether more were there.
async fn drain(
    mut cursor: ::mongodb::Cursor<Document>,
    limit: i64,
    operation: &str,
) -> Result<(Vec<Value>, bool), AdapterError> {
    let limit = limit.max(0) as usize;
    let mut documents = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|e| failed(operation, e))?
    {
        if documents.len() == limit {
            return Ok((documents, true));
        }
        documents.push(to_json(document));
    }
    Ok((documents, false))
}

fn type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::Int32(_) => "int",
        Bson::Int64(_) => "long",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Binary(_) => "binary",
        Bson::Decimal128(_) => "decimal",
        Bson::RegularExpression(_) => "regex",
        _ => "other",
    }
}

type Shape = BTreeMap<String, (BTreeSet<&'static str>, usize)>;

fn walk(document: &Document, prefix: &str, depth: usize, shape: &mut Shape) {
    for (key, value) in document {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let entry = shape.entry(path.clone()).or_default();
        entry.0.insert(type_name(value));
        entry.1 += 1;
        if let Bson::Document(nested) = value
            && depth + 1 < SHAPE_DEPTH
        {
            walk(nested, &path, depth + 1, shape);
        }
    }
}

/// The field shape of a sample: every path seen, the BSON types it held, and
/// how many of the sampled documents carried it.
fn sampled_shape(documents: &[Document]) -> Value {
    let mut shape = Shape::new();
    for document in documents {
        walk(document, "", 0, &mut shape);
    }
    Value::Array(
        shape
            .into_iter()
            .map(|(path, (types, count))| {
                json!({ "path": path, "types": types.into_iter().collect::<Vec<_>>(), "count": count })
            })
            .collect(),
    )
}

pub async fn test_mongo(conn: &pluk_store::Integration) -> Result<(), AdapterError> {
    MongoAccessor::new(mongo_config_from(conn)?).ping().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::bson::oid::ObjectId;

    #[test]
    fn extended_json_survives_the_round_trip_into_bson() {
        let oid = ObjectId::new();
        let filter = json!({ "_id": { "$oid": oid.to_hex() }, "n": 1 });
        let document = to_document(&filter, "filter").expect("parse");
        assert_eq!(document.get_object_id("_id").expect("oid"), oid);
    }

    #[test]
    fn a_non_object_filter_is_rejected() {
        let error = to_document(&json!([1, 2]), "filter").unwrap_err();
        assert!(error.message.contains("must be a JSON object"));
    }

    #[test]
    fn a_pipeline_must_be_an_array_of_stages() {
        assert!(to_pipeline(&json!({"$match": {}})).is_err());
        assert_eq!(
            to_pipeline(&json!([{"$match": {"a": 1}}]))
                .expect("pipeline")
                .len(),
            1
        );
    }

    #[test]
    fn requested_document_counts_are_clamped_to_the_cap() {
        assert_eq!(capped(None, 50), 50);
        assert_eq!(capped(Some(10_000), 50), DOCUMENT_CAP);
        assert_eq!(capped(Some(0), 50), 1);
        assert_eq!(capped(Some(-5), 50), 1);
    }

    #[test]
    fn the_sampled_shape_reports_paths_types_and_coverage() {
        let documents = vec![
            doc! { "name": "a", "meta": { "tags": 1 } },
            doc! { "name": 2 },
        ];
        let shape = sampled_shape(&documents);
        let fields = shape.as_array().expect("array");
        let name = fields
            .iter()
            .find(|f| f["path"] == json!("name"))
            .expect("name");
        assert_eq!(name["count"], json!(2));
        assert_eq!(name["types"], json!(["int", "string"]));
        assert!(fields.iter().any(|f| f["path"] == json!("meta.tags")));
    }
}
