// Casos de uso de alto nivel que consumen `DbAdapter`.
use crate::db::adapter::DbAdapter;

#[allow(dead_code)]
pub fn list_objects(adapter: &dyn DbAdapter, object_type: &str) -> Result<Vec<String>, String> {
    adapter.list_objects_by_type(object_type)
}

#[allow(dead_code)]
pub fn preview_table(
    adapter: &dyn DbAdapter,
    table: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<String>, String> {
    adapter.table_rows(table, limit, offset)
}

#[allow(dead_code)]
pub fn count_rows(adapter: &dyn DbAdapter, table: &str) -> Result<u32, String> {
    adapter.table_row_count(table)
}
