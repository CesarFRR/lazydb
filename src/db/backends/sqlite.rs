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

/// Filas con celdas expandidas (multilínea) para el inspector de fila:
/// `TEXT` con JSON → pretty de `serde_json`.
pub fn table_data_rows_pretty(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let rows = table_data_rows(path, table_name, limit, offset)?;
    Ok(crate::db::pretty::prettify_rows(rows))
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

/// Foreign keys declaradas de una tabla (`PRAGMA foreign_key_list`).
/// `to = None` significa "la PK de la tabla referenciada".
pub fn foreign_keys(path: &str, table_name: &str) -> Result<Vec<crate::db::ForeignKey>, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("PRAGMA foreign_key_list(\"{escaped}\")");
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        Ok(crate::db::ForeignKey {
            id: row.get(0)?,
            seq: row.get(1)?,
            table: row.get(2)?,
            from: row.get(3)?,
            to: row.get(4)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Offset 1-based de la primera fila donde `col == value` (para posicionar
/// el FK Jump en la tabla referenciada). `None` si no existe tal fila.
pub fn row_offset_of(
    path: &str,
    table_name: &str,
    col: &str,
    value: &str,
) -> Result<Option<u32>, DbError> {
    let conn = open_read_only(path)?;
    let t = table_name.replace('"', "\"\"");
    let c = col.replace('"', "\"\"");

    let rowid: Option<i64> = conn
        .query_row(
            &format!("SELECT rowid FROM \"{t}\" WHERE \"{c}\" = ?1 LIMIT 1"),
            [value],
            |row| row.get(0),
        )
        .optional()?;

    match rowid {
        Some(rid) => {
            let n: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM \"{t}\" WHERE rowid <= ?1"),
                [rid],
                |row| row.get(0),
            )?;
            #[allow(clippy::cast_sign_loss)]
            #[allow(clippy::cast_possible_truncation)]
            Ok(Some(n as u32))
        }
        None => Ok(None),
    }
}

fn open_read_only(path: &str) -> Result<Connection, DbError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| DbError::Open(format!("{path}: {err}")))
}

/// Query libre del usuario (modal `:`), read-only, con tope `limit`.
/// Devuelve las filas formateadas `celda | celda`.
pub fn query_free(path: &str, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();

    let rows = stmt.query_map([], |row| {
        let mut row_str = String::new();
        for i in 0..col_count {
            if i > 0 {
                row_str.push_str(" | ");
            }
            row_str.push_str(&cell_value_to_string(row, i));
        }
        Ok(row_str)
    })?;

    let mut out = Vec::new();
    for row in rows {
        if out.len() >= limit as usize {
            break;
        }
        out.push(row?);
    }
    Ok(out)
}

/// `SELECT COUNT(*)` real sobre un SQL arbitrario (`SQLite` lo optimiza
/// internamente; no materializa filas).
pub fn count_free(path: &str, sql: &str) -> Result<u32, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(sql)?;
    Ok(stmt.query_row([], |row| row.get(0))?)
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

    /// DB con dos tablas unidas por FK (escenario del FK Jump).
    fn temp_db_fk(name: &str) -> (std::path::PathBuf, impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!("lazydb_test_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("fk.db");
        let conn = Connection::open(&path).expect("abrir db temp");
        conn.execute_batch(
            "CREATE TABLE cliente (
                 id INTEGER PRIMARY KEY,
                 nombre TEXT NOT NULL
             );
             CREATE TABLE pedido (
                 id INTEGER PRIMARY KEY,
                 cliente_id INTEGER REFERENCES cliente(id),
                 nota TEXT
             );
             INSERT INTO cliente VALUES (1, 'ana'), (2, 'beso'), (3, 'cesar');
             INSERT INTO pedido VALUES (10, 2, 'urgente'), (11, NULL, 'sin cliente');",
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

    #[test]
    fn foreign_keys_enumera_la_referencia_pedido_cliente() {
        let (path, cleanup) = temp_db_fk("fk");
        let fks = foreign_keys(path.to_str().unwrap(), "pedido").expect("fk list");
        assert_eq!(fks.len(), 1);
        assert_eq!(fks[0].from, "cliente_id");
        assert_eq!(fks[0].table, "cliente");
        assert_eq!(fks[0].to.as_deref(), Some("id"));
        cleanup();
    }

    #[test]
    fn tabla_sin_foreign_keys_devuelve_lista_vacia() {
        let (path, cleanup) = temp_db_fk("fk_none");
        let fks = foreign_keys(path.to_str().unwrap(), "cliente").expect("fk list");
        assert!(fks.is_empty());
        cleanup();
    }

    #[test]
    fn row_offset_of_encuentra_la_fila_por_valor() {
        let (path, cleanup) = temp_db_fk("fk_off");
        // 'cesar' es la 3ª fila de cliente → offset 3 (1-based)
        let off = row_offset_of(path.to_str().unwrap(), "cliente", "nombre", "cesar")
            .expect("offset")
            .expect("fila existe");
        assert_eq!(off, 3);

        // Un valor inexistente → None
        let none = row_offset_of(path.to_str().unwrap(), "cliente", "nombre", "zzz")
            .expect("offset sin error");
        assert_eq!(none, None);
        cleanup();
    }

    #[test]
    fn row_offset_of_tabla_vacia_devuelve_none() {
        let (path, cleanup) = temp_db("fk_empty");
        let conn = Connection::open(&path).expect("abrir");
        conn.execute_batch("CREATE TABLE vacia (id INTEGER);").expect("schema");
        drop(conn);
        let off = row_offset_of(path.to_str().unwrap(), "vacia", "id", "1").expect("sin error");
        assert_eq!(off, None);
        cleanup();
    }
}
