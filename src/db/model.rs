//! Modelos de datos del dominio: filas y columnas tipadas.
//!
//! Antes, los backends devolvían `Vec<String>` con formato `"a | b | c"`:
//! un valor que contuviera `|` se rompía en el parseo (p.ej. el inspector
//! hacía `split('|')`). Ahora el contrato (`DbAdapter`) habla en modelos y
//! la presentación (view-model `to_lines`) es un detalle de la UI.

/// Fila de datos: celdas en el orden de `TableData::columns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<String>,
}

impl Row {
    /// Línea de presentación con un separador (view-model del Data tab).
    pub fn to_line(&self, sep: &str) -> String {
        self.cells.join(sep)
    }
}

/// Columna con su nombre y tipo declarado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub dtype: String,
}

/// Metadata completa de columna (pestaña Schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub cid: i64,
    pub name: String,
    pub dtype: String,
    pub notnull: bool,
    pub pk: bool,
}

impl ColumnInfo {
    /// Línea de presentación del Schema tab (mismo formato histórico).
    pub fn to_line(&self) -> String {
        let null_flag = if self.notnull { "NOT NULL" } else { "NULL" };
        let pk_flag = if self.pk { " PK" } else { "" };
        format!("{} | {} | {} | {null_flag}{pk_flag}", self.cid, self.name, self.dtype)
    }
}

/// Resultado de consultar una tabla/vista: columnas + filas tipadas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableData {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl TableData {
    /// View-model del Data tab: línea de cabecera + una línea por fila.
    /// `render_data_table` seguirá parseando estas líneas hasta la Fase 1
    /// (celdas 2D), pero el backend ya no depende del formato.
    pub fn to_lines(&self) -> Vec<String> {
        if self.columns.is_empty() {
            return Vec::new();
        }
        let mut lines = Vec::with_capacity(self.rows.len() + 1);
        lines.push(self.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(" | "));
        for row in &self.rows {
            lines.push(row.to_line(" | "));
        }
        lines
    }
}

impl From<ColumnInfo> for Column {
    fn from(info: ColumnInfo) -> Self {
        Self { name: info.name, dtype: info.dtype }
    }
}

/// Foreign key declarada en el esquema (`PRAGMA foreign_key_list`).
///
/// `from` = columna local · `table` = tabla referenciada · `to` = columna
/// referenciada (`None` → la PK de `table`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub id: i64,
    pub seq: i64,
    pub table: String,
    pub from: String,
    pub to: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_preserva_celdas_con_pipes() {
        // El bug que motivó los modelos: "a | b" dentro de una celda se
        // rompía al viajar como string formateado y parsearse con split.
        let row = Row { cells: vec!["a | b".to_string(), "ok".to_string()] };
        assert_eq!(row.cells[0], "a | b");
        assert_eq!(row.to_line(" | "), "a | b | ok");
    }

    #[test]
    fn table_data_to_lines_incluye_cabecera() {
        let data = TableData {
            columns: vec![
                Column { name: "id".into(), dtype: "INTEGER".into() },
                Column { name: "name".into(), dtype: "TEXT".into() },
            ],
            rows: vec![Row { cells: vec!["1".into(), "cesar".into()] }],
        };
        assert_eq!(data.to_lines(), vec!["id | name", "1 | cesar"]);
    }

    #[test]
    fn column_info_to_line_mantiene_el_formato_del_schema() {
        let c = ColumnInfo {
            cid: 0,
            name: "id".into(),
            dtype: "INTEGER".into(),
            notnull: true,
            pk: true,
        };
        assert_eq!(c.to_line(), "0 | id | INTEGER | NOT NULL PK");
    }
}
