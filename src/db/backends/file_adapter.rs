use crate::db::adapter::DbAdapter;
use crate::db::{
    Column, ColumnInfo, DbError, DbObjectHeader, DbObjectKind, ForeignKey, Row, TableData,
};

/// Adapter de archivos de datos locales (csv/tsv/parquet/json/jsonl/geojson/gpkg):
/// un archivo = un dataset virtual con una sola "tabla" (ver `file.rs`).
/// Delega en las funciones puras del driver, como los otros backends.
#[allow(dead_code)]
pub struct FileAdapter {
    path: String,
}

impl FileAdapter {
    #[allow(dead_code)]
    pub fn new(path: &str) -> Self {
        Self { path: path.to_string() }
    }

    /// Nombre del dataset único del archivo (expuesto como "tabla").
    fn dataset(&self) -> String {
        crate::db::backends::file::dataset_name(&self.path)
    }
}

impl DbAdapter for FileAdapter {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError> {
        match object_type {
            "table" => crate::db::backends::file::list_tables(&self.path),
            "view" | "index" | "trigger" => Ok(Vec::new()),
            other => Err(DbError::Sqlite(format!("tipo de objeto no soportado: {other}"))),
        }
    }

    fn list_objects(&self) -> Result<Vec<DbObjectHeader>, DbError> {
        // Un archivo = UN dataset virtual (la "tabla" del archivo).
        Ok(vec![DbObjectHeader { schema: None, nombre: self.dataset(), tipo: DbObjectKind::Table }])
    }

    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError> {
        Ok(Vec::new())
    }

    fn object_sql(&self, object_name: &str) -> Result<String, DbError> {
        if object_name == self.dataset() {
            crate::db::backends::file::object_sql(&self.path)
        } else {
            Err(DbError::Sqlite(format!("objeto desconocido: {object_name}")))
        }
    }

    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::table_columns(&self.path)
    }

    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::table_row_count(&self.path)
    }

    fn column_names(&self, table_name: &str) -> Result<Vec<Column>, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::column_names(&self.path)
    }

    fn table_data_rows(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::table_data_rows(&self.path, limit, offset)
    }

    fn table_data_rows_pretty(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::table_data_rows_pretty(&self.path, limit, offset)
    }

    fn table_rows_sorted(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
        order_col: Option<(&str, bool)>,
    ) -> Result<TableData, DbError> {
        if table_name != self.dataset() {
            return Err(DbError::Sqlite(format!("objeto desconocido: {table_name}")));
        }
        crate::db::backends::file::table_rows_sorted(&self.path, limit, offset, order_col)
    }

    fn foreign_keys(&self, _table_name: &str) -> Result<Vec<ForeignKey>, DbError> {
        Ok(Vec::new()) // los archivos planos no tienen FKs
    }

    fn row_offset_of(
        &self,
        _table_name: &str,
        _col: &str,
        _value: &str,
    ) -> Result<Option<u32>, DbError> {
        Ok(None) // sin FK jump en archivos
    }

    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
        crate::db::backends::file::query_free(&self.path, sql, limit)
    }

    fn count(&self, sql: &str) -> Result<u32, DbError> {
        crate::db::backends::file::count_free(&self.path, sql)
    }
}
