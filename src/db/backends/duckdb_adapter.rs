use crate::db::adapter::DbAdapter;
use crate::db::{ColumnInfo, DbError, TableData};

/// Adapter ligero que delega en las funciones existentes de `db::backends::duckdb`.
#[allow(dead_code)]
pub struct DuckdbAdapter {
    path: String,
}

impl DuckdbAdapter {
    #[allow(dead_code)]
    pub fn new(path: &str) -> Self {
        Self { path: path.to_string() }
    }
}

impl DbAdapter for DuckdbAdapter {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        crate::db::backends::duckdb::list_objects_by_type(&self.path, object_type)
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        crate::db::backends::duckdb::list_advanced_objects(&self.path)
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        crate::db::backends::duckdb::object_sql(&self.path, object_name)
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        crate::db::backends::duckdb::table_columns(&self.path, table_name)
    }

    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<TableData, DbError> {
        crate::db::backends::duckdb::table_rows(&self.path, table_name, limit, offset)
    }

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        crate::db::backends::duckdb::table_row_count(&self.path, table_name)
    }
}
