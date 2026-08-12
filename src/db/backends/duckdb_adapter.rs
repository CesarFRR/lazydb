use crate::db::adapter::DbAdapter;
use crate::db::{Column, ColumnInfo, DbError, DbObjectHeader, ForeignKey, Row, TableData};

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

    fn list_objects(&self) -> Result<Vec<DbObjectHeader>, DbError> {
        crate::db::backends::duckdb::list_objects(&self.path)
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

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        crate::db::backends::duckdb::table_row_count(&self.path, table_name)
    }

    fn column_names(&self, table_name: &str) -> Result<Vec<Column>, DbError> {
        crate::db::backends::duckdb::column_names(&self.path, table_name)
    }

    fn table_data_rows(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        crate::db::backends::duckdb::table_data_rows(&self.path, table_name, limit, offset)
    }

    fn table_data_rows_pretty(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        crate::db::backends::duckdb::table_data_rows_pretty(&self.path, table_name, limit, offset)
    }

    fn table_rows_sorted(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
        order_col: Option<(&str, bool)>,
    ) -> Result<TableData, DbError> {
        crate::db::backends::duckdb::table_rows_sorted(
            &self.path, table_name, limit, offset, order_col,
        )
    }

    fn foreign_keys(&self, table_name: &str) -> Result<Vec<ForeignKey>, DbError> {
        crate::db::backends::duckdb::foreign_keys(&self.path, table_name)
    }

    fn row_offset_of(
        &self,
        table_name: &str,
        col: &str,
        value: &str,
    ) -> Result<Option<u32>, DbError> {
        crate::db::backends::duckdb::row_offset_of(&self.path, table_name, col, value)
    }

    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
        crate::db::backends::duckdb::query_free(&self.path, sql, limit)
    }

    fn count(&self, sql: &str) -> Result<u32, DbError> {
        crate::db::backends::duckdb::count_free(&self.path, sql)
    }
}
