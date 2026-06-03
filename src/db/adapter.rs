// Adapter trait (contrato) que deben implementar los backends de BD.
// No tiene dependencias de UI; es el contrato de acceso a datos.
#[allow(dead_code)]
pub trait DbAdapter: Send + Sync {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, String>;
    fn list_advanced_objects(&self) -> Result<Vec<String>, String>;
    fn object_sql(&self, object_name: &str) -> Result<String, String>;
    fn table_columns(&self, table_name: &str) -> Result<Vec<String>, String>;
    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<Vec<String>, String>;
    fn table_row_count(&self, table_name: &str) -> Result<u32, String>;
}
