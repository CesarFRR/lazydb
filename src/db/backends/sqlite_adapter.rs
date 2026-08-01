use crate::db::adapter::DbAdapter;
use crate::db::{ColumnInfo, DbError, TableData};

/// Adapter ligero que delega en las funciones existentes de `db::backends::sqlite`.
#[allow(dead_code)]
pub struct SqliteAdapter {
    path: String,
}

impl SqliteAdapter {
    #[allow(dead_code)]
    pub fn new(path: &str) -> Self {
        Self { path: path.to_string() }
    }
}

impl DbAdapter for SqliteAdapter {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        crate::db::backends::sqlite::list_objects_by_type(&self.path, object_type)
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        crate::db::backends::sqlite::list_advanced_objects(&self.path)
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        crate::db::backends::sqlite::object_sql(&self.path, object_name)
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        crate::db::backends::sqlite::table_columns(&self.path, table_name)
    }

    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<TableData, DbError> {
        crate::db::backends::sqlite::table_rows(&self.path, table_name, limit, offset)
    }

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        crate::db::backends::sqlite::table_row_count(&self.path, table_name)
    }
}
