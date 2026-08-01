// Adapter trait (contrato) que deben implementar los backends de BD.
// No tiene dependencias de UI; es el contrato de acceso a datos.
use crate::db::DbError;

#[allow(dead_code)]
pub trait DbAdapter: Send + Sync {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError>;
    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError>;
    fn object_sql(&self, object_name: &str) -> Result<String, DbError>;
    fn table_columns(&self, table_name: &str) -> Result<Vec<String>, DbError>;
    fn table_rows(&self, table_name: &str, limit: u32, offset: u32)
    -> Result<Vec<String>, DbError>;
    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError>;
}
