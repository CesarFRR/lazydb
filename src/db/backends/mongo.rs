//! Backend `MongoDB` (crate oficial `mongodb`, interfaz de bajo nivel).
//!
//! Todo se maneja con `bson::Document` crudo (sin serde tipado): los datos
//! de mongo son dinámicos (cada doc puede tener campos distintos), así que
//! un struct tipado por colección no tiene sentido para un explorador.
//!
//! Modelo mental (vi-mongo):
//! - "tablas" → colecciones (`list_collection_names`)
//! - "filas" → documentos (`find` con limit/skip)
//! - "columnas" → la unión de claves observadas en los docs de una página
//! - "schema" → tipos BSON inferidos de los valores
//!
//! Todas las funciones reciben `&Client` + `db_name` (la conexión TCP es de
//! larga duración y se comparte). El adapter (`mongo_adapter.rs`) mantiene
//! el client lazy-init y bloquea con `crate::db::rt::block_on`.
//!
//! Nota API: el driver 3.8 usa "actions" (builders `IntoFuture`); cada llamada
//! se envuelve en `block_on(async { action.await })`. Los cursores se iteran
//! con `futures_util::stream::TryStreamExt`.

use crate::db::rt::block_on;
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row};

use futures_util::stream::TryStreamExt;
use mongodb::bson::{Bson, Document};
use mongodb::{Client, Database};

// ── Conexión ──────────────────────────────────────────────────────────

/// Crea un client conectado a `uri` (formato `mongodb://user:pass@host:port/db`)
/// y devuelve el client + nombre de la base parseado (vacío si la URI no
/// trae base → conexión a nivel de servidor).
pub fn connect(uri: &str) -> Result<(Client, String), DbError> {
    let client = block_on(Client::with_uri_str(uri))
        .map_err(|e| DbError::Open(format!("MongoDB ({uri}): {e}")))?;
    // El nombre de base viene en el path de la URI (`.../27017/dbname`).
    let db_name = uri
        .split('/')
        .nth(3)
        .map(ToString::to_string)
        .unwrap_or_default();
    Ok((client, db_name))
}

/// Lista las bases del servidor, excluyendo las de sistema
/// (`admin`, `config`, `local`).
pub fn list_databases(client: &Client) -> Result<Vec<String>, DbError> {
    let names = block_on(async { client.list_database_names().await })
        .map_err(|e| DbError::Open(format!("listDatabases: {e}")))?;
    Ok(names
        .into_iter()
        .filter(|d| !matches!(d.as_str(), "admin" | "config" | "local"))
        .collect())
}

fn db(client: &Client, db_name: &str) -> Database {
    client.database(db_name)
}

// ─── Catálogo (colecciones) ────────────────────────────────────────────

pub fn list_collections(client: &Client, db_name: &str) -> Result<Vec<String>, DbError> {
    let db = db(client, db_name);
    let names = block_on(async { db.list_collection_names().await })
        .map_err(|e| DbError::Open(format!("{db_name}.listCollections: {e}")))?;
    Ok(names)
}

// ─── Metadata de "columnas" (claves observadas en una página) ──────────
//
// Mongo no tiene esquema fijo. La estrategia (vi-mongo): leer la primera
// página de docs y tomar la unión ordenada de sus claves.

/// Claves observadas en los docs de la primera página, en orden de aparición.
fn observed_keys(docs: &[Document]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for doc in docs {
        for (key, _value) in doc {
            if !keys.iter().any(|k| k == key) {
                keys.push(key.clone());
            }
        }
    }
    keys
}

// ─── Render de valores BSON a texto ────────────────────────────────────

/// Render de un valor BSON a String, compacto por defecto (Data tab).
pub fn bson_to_string(v: &Bson) -> String {
    match v {
        Bson::Double(f) => format!("{f}"),
        Bson::String(s) => s.clone(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Int32(i) => i.to_string(),
        Bson::Int64(i) => i.to_string(),
        Bson::Null | Bson::Undefined => "null".to_string(),
        Bson::ObjectId(oid) => oid.to_string(),
        Bson::DateTime(dt) => dt.to_string(),
        Bson::Timestamp(ts) => ts.to_string(),
        Bson::Binary(bin) => format!("BinData(0,{})", bin.bytes.len()),
        Bson::Decimal128(d) => d.to_string(),
        Bson::Array(items) => {
            let inner: Vec<String> = items.iter().map(bson_to_string).collect();
            format!("[{}]", inner.join(", "))
        }
        Bson::Document(doc) => render_doc_compact(doc),
        other => other.to_string(),
    }
}

/// Render compacto de un documento BSON (una línea).
fn render_doc_compact(doc: &Document) -> String {
    let inner: Vec<String> = doc
        .iter()
        .map(|(k, v)| format!("{k}: {}", bson_to_string(v)))
        .collect();
    format!("{{ {} }}", inner.join(", "))
}

/// Render de un documento BSON con indentación (inspector de fila).
pub fn render_doc_pretty(doc: &Document, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    let mut out = String::from("{");
    if doc.is_empty() {
        out.push('}');
        return out;
    }
    for (k, v) in doc {
        out.push('\n');
        out.push_str(&pad);
        out.push_str("  ");
        out.push_str(k);
        out.push_str(": ");
        out.push_str(&render_value_pretty(v, indent + 1));
    }
    out.push('\n');
    out.push_str(&pad);
    out.push('}');
    out
}

fn render_value_pretty(v: &Bson, indent: usize) -> String {
    match v {
        Bson::Document(doc) => render_doc_pretty(doc, indent),
        Bson::Array(items) => {
            let pad = "  ".repeat(indent);
            if items.is_empty() {
                return "[]".to_string();
            }
            let mut out = String::from("[");
            for item in items {
                out.push('\n');
                out.push_str(&pad);
                out.push_str("  ");
                out.push_str(&render_value_pretty(item, indent + 1));
            }
            out.push('\n');
            out.push_str(&pad);
            out.push(']');
            out
        }
        other => bson_to_string(other),
    }
}

// ─── Filas (Data tab, inspector) ───────────────────────────────────────

/// Lee una página de docs como `Row`s alineadas a las claves observadas en
/// la misma página (los campos ausentes se rellenan con vacío).
async fn docs_page_async(
    client: &Client,
    db_name: &str,
    collection: &str,
    limit: u32,
    offset: u32,
    sort: Option<(&str, bool)>,
) -> Result<Vec<Row>, DbError> {
    let coll = db(client, db_name).collection::<Document>(collection);

    let mut find = coll.find(Document::new());
    if let Some((col, asc)) = sort {
        let dir = if asc { 1 } else { -1 };
        find = find.sort(Document::from_iter([(col.to_string(), Bson::Int32(dir))]));
    }
    find = find.limit(i64::from(limit)).skip(u64::from(offset));
    let mut cursor = find
        .await
        .map_err(|e| DbError::Open(format!("{collection}.find: {e}")))?;

    let mut docs: Vec<Document> = Vec::with_capacity(limit as usize);
    while let Some(doc) = cursor
        .try_next()
        .await
        .map_err(|e| DbError::Open(format!("{collection}.find cursor: {e}")))?
    {
        docs.push(doc);
        if docs.len() >= limit as usize {
            break;
        }
    }

    let keys = observed_keys(&docs);
    Ok(docs
        .iter()
        .map(|doc| {
            let cells = keys
                .iter()
                .map(|k| doc.get(k).map_or_else(String::new, bson_to_string))
                .collect();
            Row { cells }
        })
        .collect())
}

/// Igual que `docs_page_async` pero con los compuestos expandidos en
/// multilínea (inspector de fila). Cada celda `Document`/`Array` se
/// renderiza con indentación; los escalares igual que el compacto.
async fn docs_page_pretty_async(
    client: &Client,
    db_name: &str,
    collection: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let coll = db(client, db_name).collection::<Document>(collection);
    let mut cursor = coll
        .find(Document::new())
        .limit(i64::from(limit))
        .skip(u64::from(offset))
        .await
        .map_err(|e| DbError::Open(format!("{collection}.find: {e}")))?;

    let mut docs: Vec<Document> = Vec::with_capacity(limit as usize);
    while let Some(doc) = cursor
        .try_next()
        .await
        .map_err(|e| DbError::Open(format!("{collection}.find cursor: {e}")))?
    {
        docs.push(doc);
        if docs.len() >= limit as usize {
            break;
        }
    }

    let keys = observed_keys(&docs);
    Ok(docs
        .iter()
        .map(|doc| {
            let cells = keys
                .iter()
                .map(|k| {
                    doc.get(k).map_or_else(String::new, |v| render_value_pretty(v, 0))
                })
                .collect();
            Row { cells }
        })
        .collect())
}

// ─── API pública (wrappers síncronos) ──────────────────────────────────
//
// El trait `DbAdapter` es síncrono; el driver de mongo es async. Igual que
// mysql: cada función bloquea con `block_on` sobre el runtime compartido.

pub fn table_rows(
    client: &Client,
    db_name: &str,
    collection: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    block_on(docs_page_async(client, db_name, collection, limit, offset, None))
}

/// Filas con compuestos expandidos (inspector de fila).
pub fn table_data_rows_pretty(
    client: &Client,
    db_name: &str,
    collection: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    block_on(docs_page_pretty_async(client, db_name, collection, limit, offset))
}

pub fn table_rows_sorted(
    client: &Client,
    db_name: &str,
    collection: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>,
) -> Result<Vec<Row>, DbError> {
    block_on(docs_page_async(client, db_name, collection, limit, offset, order_col))
}

/// Conteo de documentos (estimatedDocumentCount — barato, sin scan).
pub fn collection_count(client: &Client, db_name: &str, collection: &str) -> Result<u32, DbError> {
    let coll = db(client, db_name).collection::<Document>(collection);
    let count = block_on(async { coll.estimated_document_count().await })
        .map_err(|e| DbError::Open(format!("{collection}.estimatedDocumentCount: {e}")))?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count as u32)
}

/// Columnas observadas en la primera página (Schema tab).
pub fn observed_columns(
    client: &Client,
    db_name: &str,
    collection: &str,
) -> Result<Vec<Column>, DbError> {
    let docs = block_on(async {
        let coll = db(client, db_name).collection::<Document>(collection);
        let mut cursor = coll
            .find(Document::new())
            .limit(20)
            .await
            .map_err(|e| DbError::Open(format!("{collection}.find: {e}")))?;
        let mut docs = Vec::new();
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| DbError::Open(format!("{collection}.find cursor: {e}")))?
        {
            docs.push(doc);
        }
        Ok::<Vec<Document>, DbError>(docs)
    })?;
    Ok(observed_keys(&docs)
        .into_iter()
        .map(|name| Column { name, dtype: "bson".into() })
        .collect())
}

/// Metadata de columnas para el inspector.
pub fn column_info(
    client: &Client,
    db_name: &str,
    collection: &str,
) -> Result<Vec<ColumnInfo>, DbError> {
    let cols = observed_columns(client, db_name, collection)?;
    Ok(cols
        .into_iter()
        .enumerate()
        .map(|(i, c)| ColumnInfo {
            #[allow(clippy::cast_possible_wrap)]
            cid: i as i64,
            name: c.name.clone(),
            dtype: "bson".to_string(),
            notnull: false,
            pk: i == 0 && c.name == "_id",
        })
        .collect())
}

/// Los FKs no existen en Mongo: siempre vacío.
pub const fn foreign_keys(
    _client: &Client,
    _db_name: &str,
    _collection: &str,
) -> Vec<ForeignKey> {
    Vec::new()
}

/// Offset del documento cuyo campo `col` serializa a `value` (para FK Jump /
/// saltar a una fila). Recorre secuencialmente: no hay índice de texto.
pub fn row_offset_of(
    client: &Client,
    db_name: &str,
    collection: &str,
    col: &str,
    value: &str,
) -> Result<Option<u32>, DbError> {
    let coll_handle = db(client, db_name).collection::<Document>(collection);
    block_on(async {
        let mut cursor = coll_handle
            .find(Document::new())
            .await
            .map_err(|e| DbError::Open(format!("{collection}.find: {e}")))?;
        let mut idx: u32 = 0;
        loop {
            let Some(doc) = cursor
                .try_next()
                .await
                .map_err(|e| DbError::Open(format!("{collection}.find cursor: {e}")))?
            else {
                return Ok(None);
            };
            let cell = doc.get(col).map_or_else(String::new, bson_to_string);
            if cell == value {
                return Ok(Some(idx));
            }
            idx += 1;
        }
    })
}

/// Query libre: Mongo no entiende SQL. Recibe un filtro JSON (`{"campo": v}`)
/// y devuelve los docs que lo cumplen como líneas compactas.
pub fn query_free(
    client: &Client,
    db_name: &str,
    filter_json: &str,
    limit: u32,
) -> Result<Vec<String>, DbError> {
    let filter = parse_filter(filter_json);
    let coll = db(client, db_name).collection::<Document>(filter_json.trim());
    block_on(async {
        let mut cursor = coll
            .find(filter)
            .limit(i64::from(limit))
            .await
            .map_err(|e| DbError::Open(format!("find: {e}")))?;
        let mut out = Vec::new();
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| DbError::Open(format!("find cursor: {e}")))?
        {
            out.push(render_doc_compact(&doc));
        }
        Ok(out)
    })
}

/// Parsea el filtro del modal de query. Si es JSON válido → se usa tal cual;
/// si no, se trata como texto libre (sin filtro, primera página).
fn parse_filter(text: &str) -> Document {
    serde_json::from_str::<serde_json::Value>(text.trim())
        .ok()
        .and_then(|v| mongodb::bson::to_bson(&v).ok())
        .and_then(|b| match b {
            Bson::Document(doc) => Some(doc),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{bson, doc};

    #[test]
    fn observed_keys_une_en_orden_de_aparicion() {
        let docs = vec![
            doc! {"a": 1, "b": "x"},
            doc! {"c": true},
            doc! {"a": 2},
        ];
        assert_eq!(observed_keys(&docs), vec!["a", "b", "c"]);
    }

    #[test]
    fn bson_to_string_renderiza_tipos_bson() {
        use mongodb::bson::oid::ObjectId;
        let oid = ObjectId::parse_str("507f1f77bcf86cd799439011").unwrap();
        assert_eq!(bson_to_string(&Bson::Int32(42)), "42");
        assert_eq!(bson_to_string(&Bson::Int64(-7)), "-7");
        assert_eq!(bson_to_string(&Bson::Double(3.5)), "3.5");
        assert_eq!(bson_to_string(&Bson::String("hola".into())), "hola");
        assert_eq!(bson_to_string(&Bson::Boolean(true)), "true");
        assert_eq!(bson_to_string(&Bson::Null), "null");
        assert_eq!(bson_to_string(&Bson::ObjectId(oid)), "507f1f77bcf86cd799439011");
        assert_eq!(bson_to_string(&bson!([1, 2, 3])), "[1, 2, 3]");
        assert_eq!(bson_to_string(&bson!({"a": 1, "b": "x"})), "{ a: 1, b: x }");
    }

    #[test]
    fn render_doc_pretty_indenta_anidados() {
        let doc = doc! {"a": 1, "nested": doc! {"b": [1, 2]}};
        let out = render_doc_pretty(&doc, 0);
        assert!(out.contains("\n  a: 1"), "doc: {out}");
        assert!(
            out.contains("\n  nested: {\n    b: [\n      1\n      2\n    ]\n  }"),
            "doc: {out}"
        );
    }

    #[test]
    fn parse_filter_acepta_json_o_vacio() {
        assert!(parse_filter("{\"a\": 1}").contains_key("a"));
        assert!(parse_filter("no es json").is_empty());
        assert!(parse_filter("").is_empty());
    }

    /// Smoke real contra `MongoDB` local (`LAZYDB_MONGO_URI`, ej.
    /// `mongodb://127.0.0.1:27017`). Crea datos de prueba en `lazydb_probe`,
    /// lista bases/colecciones y lee docs. Se ejecuta con
    /// `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "requiere MongoDB local (LAZYDB_MONGO_URI)"]
    fn smoke_real_contra_mongo_local() {
        let uri = std::env::var("LAZYDB_MONGO_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017".to_string());
        println!("=== {uri} ===");

        let (client, db_name) = connect(&uri).expect("conectar");
        println!("db_name de la URI: {db_name:?}");

        let dbs = list_databases(&client).expect("listar bases");
        println!("BASES: {dbs:?}");
        assert!(!dbs.is_empty(), "debe haber al menos una base de usuario");

        // Si la base de prueba no existe, la creamos con datos para validar
        // el read path. Ambas ramas terminan en el mismo valor: intencional.
        #[allow(clippy::branches_sharing_code)]
        let db = if dbs.iter().any(|d| d == "lazydb_probe") {
            "lazydb_probe".to_string()
        } else {
            // Creamos la base de prueba con datos para validar el read path.
            let d = client.database("lazydb_probe");
            let coll = d.collection::<Document>("smoke_probe");
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(async {
                coll.insert_one(doc! {"name": "cesar", "age": 40, "meta": doc! {"ok": true}})
                    .await
                    .expect("insert");
            });
            drop(d);
            "lazydb_probe".to_string()
        };

        let cols = list_collections(&client, &db).expect("listar colecciones");
        println!("COLECCIONES en {db}: {cols:?}");
        assert!(!cols.is_empty());

        let coll_name = cols
            .iter()
            .find(|c| c.contains("smoke") || c.contains("probe"))
            .map_or_else(|| cols[0].clone(), ToString::to_string);
        let rows = table_rows(&client, &db, &coll_name, 5, 0).expect("leer docs");
        println!("FILAS (compacto): {rows:?}");
        let pretty =
            table_data_rows_pretty(&client, &db, &coll_name, 5, 0).expect("docs pretty");
        println!("FILAS (pretty): {pretty:?}");

        let count = collection_count(&client, &db, &coll_name).expect("count");
        println!("COUNT: {count}");
        assert!(count >= 1);

        let cols_schema = observed_columns(&client, &db, &coll_name).expect("columnas");
        println!("COLUMNAS observadas: {cols_schema:?}");
        assert!(cols_schema.iter().any(|c| c.name == "_id"));
    }
}
