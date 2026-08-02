// Adapter trait (contrato) que deben implementar los backends de BD.
// No tiene dependencias de UI; es el contrato de acceso a datos.
// Habla en modelos tipados (Row/Column/TableData), nunca en strings
// formateados: ese es el consenso que todos los backends deben respetar.
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row, TableData};

#[allow(dead_code)]
pub trait DbAdapter: Send + Sync {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError>;
    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError>;
    fn object_sql(&self, object_name: &str) -> Result<String, DbError>;
    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError>;
    fn table_rows(&self, table_name: &str, limit: u32, offset: u32) -> Result<TableData, DbError>;
    fn table_row_count(&self, table_name: &str) -> Result<u32, DbError>;

    // ── extras que el controller usa directo (inspector, FK Jump, DDL) ──
    fn column_names(&self, table_name: &str) -> Result<Vec<Column>, DbError>;
    fn table_data_rows(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError>;
    fn table_rows_sorted(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
        order_col: Option<(&str, bool)>,
    ) -> Result<TableData, DbError>;
    /// Filas para el inspector de fila, con celdas "expandidas": los tipos
    /// compuestos (list/struct/map/union/array) se renderizan completos y
    /// multilínea. Default: celdas compactas (sqlite no tiene compuestos).
    fn table_data_rows_pretty(
        &self,
        table_name: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Row>, DbError> {
        self.table_data_rows(table_name, limit, offset)
    }
    fn foreign_keys(&self, table_name: &str) -> Result<Vec<ForeignKey>, DbError>;
    fn row_offset_of(
        &self,
        table_name: &str,
        col: &str,
        value: &str,
    ) -> Result<Option<u32>, DbError>;

    // ── query libre del usuario (modal `:`) ──
    /// Ejecuta un SQL arbitrario read-only y devuelve las filas formateadas
    /// (`celda | celda`), con tope `limit` (culling: nunca materializar todo).
    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError>;
    /// `SELECT COUNT(*)` sobre un SQL arbitrario (el backend lo optimiza).
    fn count(&self, sql: &str) -> Result<u32, DbError>;
}

/// Datos adicionales del contrato que algunos consumidores necesitan
/// (inspector de fila): columnas + una fila concreta por offset.
#[allow(dead_code)]
pub fn row_at(
    adapter: &dyn DbAdapter,
    table_name: &str,
    offset: u32,
) -> Result<(Vec<Column>, Row), DbError> {
    let columns = adapter.table_columns(table_name)?.into_iter().map(Into::into).collect();
    // table_rows devuelve una página; offset 0 + limit 1 basta para una fila
    let data = adapter.table_rows(table_name, 1, offset)?;
    let row = data.rows.into_iter().next().unwrap_or(Row { cells: Vec::new() });
    Ok((columns, row))
}
