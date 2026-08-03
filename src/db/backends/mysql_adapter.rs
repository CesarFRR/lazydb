use std::sync::Mutex;

use crate::db::adapter::DbAdapter;
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row, TableData};

use mysql_async::Pool;

/// Adapter MySQL/MariaDB (protocolo binario nativo vía `mysql_async`).
///
/// La pool TCP se crea en el primer uso (`lazy-init`) y se reutiliza
/// para todas las queries; sin bloqueo en la UI porque el adapter
/// corre dentro de `spawn_blocking` (query.rs).
pub struct MysqlAdapter {
    db_name: String,
    pool: Mutex<Option<Pool>>,
}

impl MysqlAdapter {
    pub fn new(url: &str) -> Result<Self, DbError> {
        let (pool, db_name) = crate::db::backends::mysql::connect(url)?;
        Ok(Self { db_name, pool: Mutex::new(Some(pool)) })
    }

    // El MutexGuard debe vivir mientras `f` ejecuta: no droppear temprano.
    #[allow(clippy::significant_drop_in_scrutinee)]
    fn with_pool<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Pool, &str) -> Result<T, DbError>,
    {
        self.pool
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| DbError::Open("conexión MySQL ya cerrada".into()))
            .and_then(|pool_ref| f(pool_ref, &self.db_name))
    }
}

impl DbAdapter for MysqlAdapter {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        self.with_pool(|pool, db| match object_type {
            "table" => crate::db::backends::mysql::list_tables(pool, db),
            "view" => crate::db::backends::mysql::list_views(pool, db),
            "index" => crate::db::backends::mysql::list_all_indexes(pool, db),
            "trigger" => crate::db::backends::mysql::list_triggers(pool, db),
            _ => Ok(Vec::new()),
        })
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        // Para MySQL/MariaDB: ver ambas categorías juntas (índices y triggers)
        self.with_pool(|pool, db| {
            let mut out = crate::db::backends::mysql::list_all_indexes(pool, db)?;
            out.extend(crate::db::backends::mysql::list_triggers(pool, db)?);
            Ok(out)
        })
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::object_sql(pool, db, object_name))
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::column_info(pool, db, table_name))
    }

    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<TableData, DbError> {
        self.with_pool(|pool, db| {
            crate::db::backends::mysql::table_rows_sorted(pool, db, table_name, limit, offset, None)
        })
    }

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::table_row_count(pool, db, table_name))
    }

    fn column_names(&self, table_name: &str) -> Result<Vec<Column>, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::column_names(pool, db, table_name))
    }

    fn table_data_rows(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        self.with_pool(|pool, db| {
            crate::db::backends::mysql::table_data_rows(pool, db, table_name, limit, offset)
        })
    }

    fn table_rows_sorted(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
        order_col: Option<(&str, bool)>,
    ) -> Result<TableData, DbError> {
        self.with_pool(|pool, db| {
            crate::db::backends::mysql::table_rows_sorted(
                pool, db, table_name, limit, offset, order_col,
            )
        })
    }

    fn foreign_keys(&self, table_name: &str) -> Result<Vec<ForeignKey>, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::foreign_keys(pool, db, table_name))
    }

    fn row_offset_of(
        &self,
        table_name: &str,
        col: &str,
        value: &str,
    ) -> Result<Option<u32>, DbError> {
        self.with_pool(|pool, db| {
            crate::db::backends::mysql::row_offset_of(pool, db, table_name, col, value)
        })
    }

    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::query_free(pool, db, sql, limit))
    }

    fn count(&self, sql: &str) -> Result<u32, DbError> {
        self.with_pool(|pool, db| crate::db::backends::mysql::count_free(pool, db, sql))
    }
}
