use std::sync::Mutex;

use crate::db::adapter::DbAdapter;
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row, TableData};

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
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        self.with_client(|client, db| match object_type {
            "table" | "collection" => {
                crate::db::backends::mongo::list_collections(client, db)
            }
            // Mongo no tiene vistas SQL ni triggers ni índices separados
            _ => Ok(Vec::new()),
        })
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        Ok(Vec::new())
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        // No hay DDL en Mongo: devolvemos una descripción del objeto.
        Ok(format!("// Colección MongoDB: {object_name}\n// Mongo no tiene DDL; los documentos son bson::Document libres."))
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::column_info(client, db, table_name)
        })
    }

    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<TableData, DbError> {
        self.with_client(|client, db| {
            let columns =
                crate::db::backends::mongo::observed_columns(client, db, table_name)?;
            let rows =
                crate::db::backends::mongo::table_rows(client, db, table_name, limit, offset)?;
            Ok(TableData { columns, rows })
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
            crate::db::backends::mongo::table_rows(client, db, table_name, limit, offset)
        })
    }

    fn table_data_rows_pretty(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::table_data_rows_pretty(client, db, table_name, limit, offset)
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
            let columns =
                crate::db::backends::mongo::observed_columns(client, db, table_name)?;
            let rows = crate::db::backends::mongo::table_rows_sorted(
                client, db, table_name, limit, offset, order_col,
            )?;
            Ok(TableData { columns, rows })
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

    fn row_inspector_pairs(
        &self,
        object_name: &str,
        offset: u32,
    ) -> Option<Vec<(String, String)>> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::row_inspector_pairs(client, db, object_name, offset)
        })
        .ok()
    }

    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
        self.with_client(|client, db| {
            crate::db::backends::mongo::query_free(client, db, sql, limit)
        })
    }

    fn count(&self, _sql: &str) -> Result<u32, DbError> {
        // COUNT(*) de un find: usamos el conteo de la colección completa.
        // El SQL no es real en mongo; el modal `:` de lazydb no aplica.
        Ok(0)
    }
}
