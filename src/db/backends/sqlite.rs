use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::db::{Column, ColumnInfo, DbError, Row, TableData};

pub fn list_objects_by_type(path: &str, object_type: &str) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type = ?1
         AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;

    let rows = stmt.query_map([object_type], |row| row.get::<_, String>(0))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }

    Ok(out)
}

pub fn list_advanced_objects(path: &str) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(
        "SELECT type, name FROM sqlite_master
         WHERE type IN ('index','trigger')
         AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name",
    )?;

    let rows = stmt.query_map([], |row| {
        let kind: String = row.get(0)?;
        let name: String = row.get(1)?;
        Ok(format!("{kind}:{name}"))
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }

    Ok(out)
}

pub fn object_sql(path: &str, object_name: &str) -> Result<String, DbError> {
    let conn = open_read_only(path)?;
    let escaped = object_name.replace('"', "\"\"");
    let sql = format!(
        "SELECT sql FROM sqlite_master WHERE name = \"{escaped}\" ORDER BY CASE type WHEN 'table' THEN 1 WHEN 'view' THEN 2 WHEN 'index' THEN 3 WHEN 'trigger' THEN 4 ELSE 9 END LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;

    let ddl: Option<String> = stmt.query_row([], |row| row.get(0)).optional()?;

    Ok(ddl.unwrap_or_else(|| "-- SQL no disponible para este objeto".to_string()))
}

#[allow(dead_code)]
pub fn list_objects(path: &str) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type IN ('table','view')
         AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;

    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }

    Ok(out)
}

/// Columnas (nombre + tipo declarado) de una tabla. Para inspector de fila.
pub fn column_names(path: &str, table_name: &str) -> Result<Vec<Column>, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("PRAGMA table_info(\"{escaped}\")");
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let dtype: String = row.get(2)?;
        Ok(Column { name, dtype })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Metadata completo de columna (cid, name, type, nullability). Para pestaña
/// Schema; `ColumnInfo::to_line()` formatea la línea de presentación.
pub fn table_columns(path: &str, table_name: &str) -> Result<Vec<ColumnInfo>, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("PRAGMA table_info(\"{escaped}\")");
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        let cid: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let dtype: String = row.get(2)?;
        let notnull: i64 = row.get(3)?;
        let pk: i64 = row.get(5)?;
        Ok(ColumnInfo { cid, name, dtype, notnull: notnull == 1, pk: pk == 1 })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }

    Ok(out)
}

/// Filas de datos SIN header. Para inspector y preview.
pub fn table_data_rows(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql)?;

    let col_count = stmt.column_names().len();

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..col_count {
            values.push(cell_value_to_string(row, i));
        }
        Ok(Row { cells: values })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }

    Ok(out)
}

/// Filas + columnas, para el preview de datos.
pub fn table_rows(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<TableData, DbError> {
    table_rows_sorted(path, table_name, limit, offset, None)
}

/// Filas + columnas, con ORDER BY opcional. El contrato del dominio habla
/// en modelos (`TableData`), no en strings formateados.
pub fn table_rows_sorted(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>, // (column_name, asc)
) -> Result<TableData, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let order_clause = if let Some((col, asc)) = order_col {
        let col_esc = col.replace('"', "\"\"");
        let dir = if asc { "ASC" } else { "DESC" };
        format!(" ORDER BY \"{col_esc}\" {dir}")
    } else {
        String::new()
    };
    let sql = format!("SELECT * FROM \"{escaped}\"{order_clause} LIMIT {limit} OFFSET {offset}");

    let mut stmt = conn.prepare(&sql)?;
    let col_count = stmt.column_count();
    // El dtype declarado lo da PRAGMA table_info (pestaña Schema); aquí el
    // modelo viaja con el nombre real de cada columna.
    let columns = (0..col_count)
        .map(|i| Column {
            name: stmt.column_name(i).unwrap_or("?").to_string(),
            dtype: String::new(),
        })
        .collect::<Vec<_>>();

    if columns.is_empty() {
        return Ok(TableData { columns, rows: Vec::new() });
    }

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..col_count {
            values.push(cell_value_to_string(row, i));
        }
        Ok(Row { cells: values })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(TableData { columns, rows: out })
}

pub fn table_row_count(path: &str, table_name: &str) -> Result<u32, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT COUNT(*) FROM \"{escaped}\"");
    let mut stmt = conn.prepare(&sql)?;

    let count: u32 = stmt.query_row([], |row| row.get(0))?;

    Ok(count)
}

fn open_read_only(path: &str) -> Result<Connection, DbError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| DbError::Open(format!("{path}: {err}")))
}

/// Convierte una celda a String según el tipo de valor almacenado
/// (entero, real, texto, nulo, blob).
///
/// `row.get::<_, String>` falla para columnas numéricas (rusqlite no
/// convierte INTEGER/REAL a String) y producía `[NULL]` falsos en todas
/// las celdas numéricas de la UI.
pub fn cell_value_to_string(row: &rusqlite::Row<'_>, i: usize) -> String {
    use rusqlite::types::ValueRef;
    match row.get_ref(i) {
        Ok(ValueRef::Null) => "[NULL]".to_string(),
        Ok(ValueRef::Integer(v)) => v.to_string(),
        Ok(ValueRef::Real(v)) => v.to_string(),
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
        Ok(ValueRef::Blob(_)) => "<blob>".to_string(),
        Err(e) => format!("<error: {e}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DB temporal con una tabla que incluye un valor con pipes dentro de
    /// una celda: el bug que motivó los modelos tipados.
    fn temp_db(name: &str) -> (std::path::PathBuf, impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!("lazydb_test_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("model.db");
        let conn = Connection::open(&path).expect("abrir db temp");
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER, note TEXT);
             INSERT INTO t VALUES (1, 'a | b'), (2, 'ok');",
        )
        .expect("schema");
        drop(conn);
        let cleanup_path = path.clone();
        let cleanup = move || {
            let _ = std::fs::remove_file(&cleanup_path);
            let _ = std::fs::remove_dir(&dir);
        };
        (path, cleanup)
    }

    #[test]
    fn table_rows_sorted_devuelve_modelos_con_celdas_intactas() {
        let (path, cleanup) = temp_db("model");
        let data =
            table_rows_sorted(path.to_str().unwrap(), "t", 10, 0, None).expect("consultar tabla");

        assert_eq!(data.columns.len(), 2);
        assert_eq!(data.columns[0].name, "id");
        assert_eq!(data.columns[1].name, "note");

        assert_eq!(data.rows.len(), 2);
        // La celda con pipes viaja intacta: esto se rompía con split('|')
        assert_eq!(data.rows[0].cells, vec!["1", "a | b"]);
        assert_eq!(data.rows[1].cells, vec!["2", "ok"]);

        cleanup();
    }

    #[test]
    fn table_row_count_devuelve_el_total_real() {
        let (path, cleanup) = temp_db("model_count");
        assert_eq!(table_row_count(path.to_str().unwrap(), "t"), Ok(2));
        cleanup();
    }
}
