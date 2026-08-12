// Adapter trait (contrato) que deben implementar los backends de BD.
// No tiene dependencias de UI; es el contrato de acceso a datos.
// Habla en modelos tipados (Row/Column/TableData), nunca en strings
// formateados: ese es el consenso que todos los backends deben respetar.
use crate::db::{Column, ColumnInfo, DbError, DbObjectHeader, ForeignKey, Row, TableData};

pub trait DbAdapter: Send + Sync {
    fn list_objects_by_type(&self, object_type: &str) -> Result<Vec<String>, DbError>;
    /// Objetos avanzados (índices/triggers) en formato `tipo:nombre`.
    /// El `list_objects` tipado lo reemplazó en el controller; se mantiene
    /// como parte del contrato (lo usan los smoke tests de los backends).
    #[allow(dead_code)]
    fn list_advanced_objects(&self) -> Result<Vec<String>, DbError>;
    /// Catálogo completo de objetos en UNA consulta por motor (o el mínimo
    /// de llamadas que el motor permita): tablas/vistas/índices/triggers/
    /// colecciones con su tipo. Es la fuente para el árbol lateral SIN
    /// quedarse pegado (una sola ida al servidor, en vez de 3-4 queries
    /// separadas que además reconstruyen DDL innecesariamente, p.ej.
    /// `pg_indexes` llama a `pg_get_indexdef` por fila).
    #[allow(dead_code)] // lo consumirá el lazy catalog (controller) en la Ronda 2
    fn list_objects(&self) -> Result<Vec<DbObjectHeader>, DbError>;
    fn object_sql(&self, object_name: &str) -> Result<String, DbError>;
    fn table_columns(&self, table_name: &str) -> Result<Vec<ColumnInfo>, DbError>;
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

    // ── inspector de fila (NoSQL) ──
    /// ¿Este backend es `NoSQL` (documentos/clave-valor) en vez de SQL?
    /// La UI usa esto para cambiar la terminología (`row` → `doc`), mostrar
    /// el toggle JSON del modal, etc. SQL devuelve `false` por defecto.
    fn is_nosql(&self) -> bool {
        false
    }

    /// Pares `(clave, valor)` de la fila en `offset` para el modal de
    /// detalles. SOLO incluye los campos PRESENTES en el documento/entidad:
    /// en `NoSQL` (mongo) cada fila puede tener campos distintos y los
    /// ausentes no deben aparecer (ni como fila vacía ni desalineados).
    ///
    /// SQL (esquema fijo) devuelve `None` → el inspector usa el flujo clásico
    /// `column_names` + `table_data_rows_pretty` alineados por índice.
    fn row_inspector_pairs(
        &self,
        _object_name: &str,
        _offset: u32,
    ) -> Option<Vec<(String, String)>> {
        None
    }

    /// JSON pretty del documento en `offset` (modo JSON del modal de
    /// detalles). `NoSQL` (mongo) lo implementa; SQL devuelve `None`.
    fn row_inspector_json(&self, _object_name: &str, _offset: u32) -> Option<String> {
        None
    }

    // ── query libre del usuario (modal `:`) ──
    /// Ejecuta un SQL arbitrario read-only y devuelve las filas formateadas
    /// (`celda | celda`), con tope `limit` (culling: nunca materializar todo).
    fn query(&self, sql: &str, limit: u32) -> Result<Vec<String>, DbError>;
    /// `SELECT COUNT(*)` sobre un SQL arbitrario (el backend lo optimiza).
    fn count(&self, sql: &str) -> Result<u32, DbError>;
}
