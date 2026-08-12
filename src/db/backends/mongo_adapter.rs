use std::sync::Mutex;

use crate::db::adapter::DbAdapter;
use crate::db::{Column, ColumnInfo, DbError, DbObjectHeader, ForeignKey, Row, TableData};

use mongodb::Client;

/// Adapter `MongoDB` (crate oficial, interfaz de bajo nivel con
/// `bson::Document` crudo).
///
/// El client TCP se crea en el primer uso (`lazy-init`) y se reutiliza
/// para todas las operaciones. El adapter corre dentro de `spawn_blocking`
/// (query.rs) y bloquea con `block_on` sobre el runtime compartido, igual
/// que mysql/postgres.
pub struct MongoAdapter {
    db_name: String,
    client: Mutex<Option<Client>>,
}

impl MongoAdapter {
    pub fn new(uri: &str) -> Result<Self, DbError> {
        let (client, db_name) = crate::db::backends::mongo::connect(uri)?;
        Ok(Self { db_name, client: Mutex::new(Some(client)) })
    }

    #[allow(clippy::significant_drop_in_scrutinee)]
    fn with_client<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Client, &str) -> Result<T, DbError>,
    {
        self.client
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| DbError::Open("conexión MongoDB ya cerrada".into()))
            .and_then(|client| f(client, &self.db_name))
    }
}

impl DbAdapter for MongoAdapter {
    fn is_nosql(&self) -> bool {
        true
    }

    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        self.with_client(|client, db| match object_type {
            "table" | "collection" => crate::db::backends::mongo::list_collections(client, db),
            // Mongo no tiene vistas SQL ni triggers ni índices separados
            _ => Ok(Vec::new()),
        })
    }

    fn list_objects(&self) -> Result<Vec<DbObjectHeader>, DbError> {
        self.with_client(crate::db::backends::mongo::list_objects)
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        Ok(Vec::new())
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        // No hay DDL en Mongo: devolvemos una descripción del objeto.
        Ok(format!(
            "// Colección MongoDB: {object_name}\n// Mongo no tiene DDL; los documentos son bson::Document libres."
        ))
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::column_info(client, db, table_name)
        })
    }

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::collection_count(client, db, table_name)
        })
    }

    fn column_names(&self, table_name: &str) -> Result<Vec<Column>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::observed_columns(client, db, table_name)
        })
    }

    fn table_data_rows(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::table_rows_sorted(
                client, db, table_name, limit, offset, None,
            )
            .map(|data| data.rows)
        })
    }

    fn table_data_rows_pretty(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::table_data_rows_pretty(
                client, db, table_name, limit, offset,
            )
        })
    }

    fn table_rows_sorted(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
        order_col: Option<(&str, bool)>,
    ) -> Result<TableData, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::table_rows_sorted(
                client, db, table_name, limit, offset, order_col,
            )
        })
    }

    fn foreign_keys(&self, table_name: &str) -> Result<Vec<ForeignKey>, DbError> {
        self.with_client(|client, db| {
            Ok(crate::db::backends::mongo::foreign_keys(client, db, table_name))
        })
    }

    fn row_offset_of(
        &self,
        table_name: &str,
        col: &str,
        value: &str,
    ) -> Result<Option<u32>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::row_offset_of(client, db, table_name, col, value)
        })
    }

    fn row_inspector_pairs(&self, object_name: &str, offset: u32) -> Option<Vec<(String, String)>> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::row_inspector_pairs(client, db, object_name, offset)
                .map(|(pairs, _json)| pairs)
        })
        .ok()
    }

    fn row_inspector_json(&self, object_name: &str, offset: u32) -> Option<String> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::row_inspector_pairs(client, db, object_name, offset)
                .map(|(_pairs, json)| json)
        })
        .ok()
    }

    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
        self.with_client(|client, db| {
            let (coll, filter) = split_collection_filter(sql);
            if coll.is_empty() {
                return Err(DbError::Open("selecciona una colección antes de filtrar".into()));
            }
            crate::db::backends::mongo::query_free(client, db, &coll, &filter, limit)
        })
    }

    fn count(&self, sql: &str) -> Result<u32, DbError> {
        self.with_client(|client, db| {
            let (coll, filter) = split_collection_filter(sql);
            if coll.is_empty() {
                return Ok(0);
            }
            crate::db::backends::mongo::count_free(client, db, &coll, &filter)
        })
    }
}

/// El sql interno de mongo es `@<coleccion> <filtro JSON>` (lo inyecta el
/// controller en `execute_user_query`). Separa ambos; si no hay prefijo
/// `@`, colección vacía (error controlado por el caller).
fn split_collection_filter(sql: &str) -> (String, String) {
    if let Some(rest) = sql.strip_prefix('@') {
        let trimmed = rest.trim_start();
        if let Some(space) = trimmed.find(char::is_whitespace) {
            let (coll, filter) = trimmed.split_at(space);
            return (coll.to_string(), filter.trim().to_string());
        }
        return (trimmed.to_string(), String::new());
    }
    (String::new(), sql.to_string())
}

#[cfg(test)]
mod tests {
    use super::split_collection_filter;

    #[test]
    fn split_separa_coleccion_y_filtro() {
        assert_eq!(
            split_collection_filter("@test_probe {\"name\": \"cesar\"}"),
            ("test_probe".to_string(), "{\"name\": \"cesar\"}".to_string())
        );
    }

    #[test]
    fn split_sin_filtro_devuelve_solo_coleccion() {
        assert_eq!(
            split_collection_filter("@test_probe"),
            ("test_probe".to_string(), String::new())
        );
    }

    #[test]
    fn split_sin_prefijo_devuelve_coleccion_vacia() {
        assert_eq!(
            split_collection_filter("{\"name\": \"x\"}"),
            (String::new(), "{\"name\": \"x\"}".to_string())
        );
    }
}
