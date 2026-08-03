use duckdb::{Connection, OptionalExt, types::ValueRef};

use crate::db::{Column, ColumnInfo, DbError, Row, TableData};

/// Lista objetos por tipo ('table', 'view', 'index', 'trigger').
/// `DuckDB` expone el catálogo vía `information_schema` (tablas) y
/// `duckdb_views()`/`duckdb_indexes()` para el resto. Desde 1.5.x:
/// - las vistas internas (`duckdb_*`, `sqlite_master`, ...) viven en schema
///   `main` y se filtran con la columna `internal` de `duckdb_views()`;
/// - los triggers ya NO existen en el motor (`duckdb_triggers()` fue removida),
///   por lo que "trigger" devuelve siempre una lista vacía.
pub fn list_objects_by_type(path: &str, object_type: &str) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let sql = match object_type {
        "table" => {
            "SELECT table_name FROM information_schema.tables
                    WHERE table_schema = 'main' AND table_type = 'BASE TABLE'
                    ORDER BY table_name"
        }
        "view" => {
            "SELECT view_name FROM duckdb_views()
                    WHERE schema_name = 'main' AND NOT internal
                    ORDER BY view_name"
        }
        "index" => {
            "SELECT index_name FROM duckdb_indexes()
                    WHERE schema_name = 'main'
                    ORDER BY index_name"
        }
        // DuckDB 1.5.x no soporta triggers (duckdb_triggers() ya no existe).
        "trigger" => return Ok(Vec::new()),
        other => return Err(DbError::Sqlite(format!("tipo de objeto no soportado: {other}"))),
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Objetos avanzados (índices y triggers) en formato `tipo:nombre`.
/// Solo índices: `DuckDB` 1.5.x eliminó los triggers.
pub fn list_advanced_objects(path: &str) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(
        "SELECT 'index:' || index_name FROM duckdb_indexes() WHERE schema_name = 'main'
         ORDER BY 1",
    )?;

    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// DDL de un objeto. `DuckDB` guarda el SQL original en `duckdb_tables()` /
/// `duckdb_views()` (no hay `sqlite_master`).
pub fn object_sql(path: &str, object_name: &str) -> Result<String, DbError> {
    let conn = open_read_only(path)?;

    // 1) Tabla → SQL original del catálogo
    let mut stmt = conn.prepare(
        "SELECT sql FROM duckdb_tables()
         WHERE schema_name = 'main' AND table_name = ?1",
    )?;
    let table_sql: Option<String> = stmt.query_row([object_name], |row| row.get(0)).optional()?;
    if let Some(sql) = table_sql {
        return Ok(sql);
    }

    // 2) View → definición del catálogo
    let mut stmt = conn.prepare(
        "SELECT view_definition FROM duckdb_views()
         WHERE schema_name = 'main' AND view_name = ?1",
    )?;
    let view_sql: Option<String> = stmt.query_row([object_name], |row| row.get(0)).optional()?;
    if let Some(def) = view_sql {
        return Ok(format!("CREATE VIEW \"{object_name}\" AS {def}"));
    }

    Ok("-- SQL no disponible para este objeto".to_string())
}

/// Columnas (nombre + tipo declarado) de una tabla. Para inspector de fila.
#[allow(dead_code)]
pub fn column_names(path: &str, table_name: &str) -> Result<Vec<Column>, DbError> {
    let conn = open_read_only(path)?;
    column_names_conn(&conn, table_name)
}

/// Igual que `column_names` pero sobre la conexión ya abierta.
/// IMPORTANTE: usar la MISMA conexión que luego ejecuta el SELECT. Con la
/// extensión `spatial` cargada (gpkg/geojson), abrir una segunda conexión
/// concurrente al mismo archivo provocaba
/// `TransactionContext::ActiveTransaction called without active transaction`
/// al consultar filas (misma lección que file.rs).
fn column_names_conn(conn: &Connection, table_name: &str) -> Result<Vec<Column>, DbError> {
    let escaped = table_name.replace('\'', "''");
    let sql = format!("PRAGMA table_info('{escaped}')");
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
    let escaped = table_name.replace('\'', "''");
    let sql = format!("PRAGMA table_info('{escaped}')");
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        let cid: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let dtype: String = row.get(2)?;
        let notnull: bool = row.get(3)?;
        let pk: bool = row.get(5)?;
        Ok(ColumnInfo { cid, name, dtype, notnull, pk })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Filas de datos SIN header. Para inspector y preview.
#[allow(dead_code)]
pub fn table_data_rows(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    rows_impl(path, table_name, limit, offset, false)
}

/// Filas con celdas expandidas (multilínea) para el inspector de fila: los
/// tipos compuestos se muestran completos (list/struct/map/union/array).
#[allow(dead_code)]
pub fn table_data_rows_pretty(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    rows_impl(path, table_name, limit, offset, true)
}

fn rows_impl(
    path: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
    pretty: bool,
) -> Result<Vec<Row>, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql)?;

    // Ejecutamos ANTES de pedir metadatos: con la extensión `spatial` (geojson/
    // gpkg), consultar el catálogo mientras el SELECT está preparado (aunque
    // sea en la misma conexión) rompe la transacción con
    // `ActiveTransaction called without active transaction`. Los nombres y el
    // count se obtienen de las rows YA ejecutadas (duckdb-rs `Rows`).
    let mut rows = stmt.query([])?;
    let col_count = rows.as_ref().expect("stmt").column_count();
    let col_names = rows.as_ref().expect("stmt").column_names();

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() >= limit as usize {
            break;
        }
        let mut values = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let cell =
                if pretty { cell_value_to_pretty(row, i) } else { cell_value_to_string(row, i) };
            values.push(cell);
        }
        out.push(Row { cells: values });
    }
    let _ = col_names; // el count es lo que importa aquí; los nombres vía catálogo
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

    // Ejecutamos ANTES de pedir metadatos: con la extensión `spatial`
    // (geojson/gpkg), consultar el catálogo mientras el SELECT está preparado
    // rompe la transacción (`ActiveTransaction called without active
    // transaction`). Los nombres y el count vienen de las rows ejecutadas
    // (duckdb-rs `Rows::column_names`).
    let mut rows = stmt.query([])?;
    let col_count = rows.as_ref().expect("stmt").column_count();
    let columns: Vec<Column> = rows
        .as_ref()
        .expect("stmt")
        .column_names()
        .into_iter()
        .map(|name| Column { name, dtype: String::new() })
        .collect();
    if columns.is_empty() {
        return Ok(TableData { columns, rows: Vec::new() });
    }

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() >= limit as usize {
            break;
        }
        let mut values = Vec::with_capacity(col_count);
        for i in 0..col_count {
            values.push(cell_value_to_string(row, i));
        }
        out.push(Row { cells: values });
    }
    Ok(TableData { columns, rows: out })
}

pub fn table_row_count(path: &str, table_name: &str) -> Result<u32, DbError> {
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT COUNT(*) FROM \"{escaped}\"");
    let mut stmt = conn.prepare(&sql)?;

    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    #[allow(clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_truncation)]
    Ok(count as u32)
}

/// Foreign keys declaradas de una tabla. `DuckDB` NO soporta
/// `PRAGMA foreign_key_list`; el catálogo está en `duckdb_constraints()` y
/// las columnas son de tipo List → las leemos con `array_to_string()`.
/// `to = None` significa "la PK de la tabla referenciada".
#[allow(dead_code)]
pub fn foreign_keys(path: &str, table_name: &str) -> Result<Vec<crate::db::ForeignKey>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(
        "SELECT array_to_string(constraint_column_names, ','),
                referenced_table,
                array_to_string(referenced_column_names, ',')
         FROM duckdb_constraints()
         WHERE constraint_type = 'FOREIGN KEY'
           AND table_name = ?1",
    )?;

    let mut out = Vec::new();
    let rows = stmt.query_map([table_name], |row| {
        let from_names: String = row.get(0)?;
        let table: String = row.get(1)?;
        let to_names: String = row.get(2)?;
        // Formato '{col1,col2}'; tomamos el primero si es una FK compuesta.
        let first =
            |s: &str| s.trim_matches(['{', '}']).split(',').next().unwrap_or("").to_string();
        Ok(crate::db::ForeignKey {
            id: 0,
            seq: 0,
            table,
            from: first(&from_names),
            to: Some(first(&to_names)),
        })
    })?;

    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Offset 1-based de la primera fila donde `col == value` (para posicionar
/// el FK Jump en la tabla referenciada). `None` si no existe tal fila.
///
/// `DuckDB` no tiene rowid: usamos `ROW_NUMBER()` sobre el scan natural.
#[allow(dead_code)]
pub fn row_offset_of(
    path: &str,
    table_name: &str,
    col: &str,
    value: &str,
) -> Result<Option<u32>, DbError> {
    let conn = open_read_only(path)?;
    let t = table_name.replace('"', "\"\"");
    let c = col.replace('"', "\"\"");

    let rn: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT rn FROM (SELECT \"{c}\", ROW_NUMBER() OVER () AS rn FROM \"{t}\") sub WHERE \"{c}\" = ?1 LIMIT 1"
            ),
            [value],
            |row| row.get(0),
        )
        .optional()?;

    #[allow(clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_truncation)]
    Ok(rn.map(|n| n as u32))
}

/// Apertura read-only: `DuckDB` usa `Config::access_mode(ReadOnly)` en vez de
/// `OpenFlags` como rusqlite.
fn open_read_only(path: &str) -> Result<Connection, DbError> {
    let config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .map_err(|err| DbError::Open(format!("config {path}: {err}")))?;
    Connection::open_with_flags(path, config).map_err(|err| DbError::Open(format!("{path}: {err}")))
}

/// Query libre del usuario (modal `:`), read-only, con tope `limit`.
/// Devuelve las filas formateadas `celda | celda`.
pub fn query_free(path: &str, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(sql)?;

    // duckdb-rs: column_count() panica si la query no se ejecutó → primero
    // `query()` (ejecuta). El count se pide vía `rows.as_ref()` para no
    // romper el borrow del stmt (ver doc de duckdb column.rs).
    let mut rows = stmt.query([])?;
    let col_count = rows.as_ref().expect("stmt").column_count();

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() >= limit as usize {
            break;
        }
        let mut row_str = String::new();
        for i in 0..col_count {
            if i > 0 {
                row_str.push_str(" | ");
            }
            row_str.push_str(&cell_value_to_string(row, i));
        }
        out.push(row_str);
    }
    Ok(out)
}

/// `SELECT COUNT(*)` real sobre un SQL arbitrario (`DuckDB` lo optimiza).
pub fn count_free(path: &str, sql: &str) -> Result<u32, DbError> {
    let conn = open_read_only(path)?;
    let mut stmt = conn.prepare(sql)?;
    let n: i64 = stmt.query_row([], |row| row.get(0))?;
    #[allow(clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_truncation)]
    Ok(n as u32)
}

/// Convierte una celda a String según el tipo de valor almacenado.
/// DuckDB/arrow expone más variantes que rusqlite (Boolean, Decimal,
/// Date32, Time64, Timestamp, List, Struct, etc.); las que no son de
/// bajo nivel (List/Struct/Map/Array/Union) se muestran como "<{type}>"
/// para no inundar la UI.
pub fn cell_value_to_string(row: &duckdb::Row<'_>, i: usize) -> String {
    match row.get_ref(i) {
        Ok(ValueRef::Null) => "[NULL]".to_string(),
        Ok(ValueRef::Boolean(v)) => v.to_string(),
        Ok(ValueRef::TinyInt(v)) => v.to_string(),
        Ok(ValueRef::SmallInt(v)) => v.to_string(),
        Ok(ValueRef::Int(v)) => v.to_string(),
        Ok(ValueRef::BigInt(v)) => v.to_string(),
        Ok(ValueRef::HugeInt(v)) => v.to_string(),
        Ok(ValueRef::UTinyInt(v)) => v.to_string(),
        Ok(ValueRef::USmallInt(v)) => v.to_string(),
        Ok(ValueRef::UInt(v)) => v.to_string(),
        Ok(ValueRef::UBigInt(v)) => v.to_string(),
        Ok(ValueRef::Float(v)) => v.to_string(),
        Ok(ValueRef::Double(v)) => v.to_string(),
        Ok(ValueRef::Decimal(v)) => v.to_string(),
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
        Ok(ValueRef::Blob(b)) => format!("0x{}", hex(b)),
        Ok(ValueRef::Geometry(b)) => format!("WKB[{}B]", b.len()),
        Ok(ValueRef::Date32(d)) => {
            format!("{y:04}-{:02}-{:02}", month(d), day(d), y = year(d))
        }
        Ok(ValueRef::Time64(tu, v)) => time_to_string(tu, v),
        Ok(ValueRef::Timestamp(tu, v)) => timestamp_to_string(tu, v),
        Ok(ValueRef::Interval { months, days, nanos }) => interval_to_string(months, days, nanos),
        Ok(ValueRef::Enum(..)) => row
            .get_ref(i)
            .ok()
            .and_then(|r| r.as_str().ok())
            .map_or_else(|| "<enum>".to_string(), ToOwned::to_owned),
        Ok(ValueRef::List(l, i)) => format!("<list[{}]>", list_len(l, i)),
        Ok(ValueRef::Struct(_, _)) => "<struct>".to_string(),
        Ok(ValueRef::Map(_, _)) => "<map>".to_string(),
        Ok(ValueRef::Union(_, _)) => "<union>".to_string(),
        Ok(ValueRef::Array(_, _)) => "<array>".to_string(),
        // ValueRef es non-exhaustive en duckdb 1.10505: variantes futuras
        Ok(_) => "<otro>".to_string(),
        Err(e) => format!("<error: {e}>"),
    }
}

/// Celda expandida (multilínea) para el inspector de fila: convierte el
/// `ValueRef` a `Value` owned y renderiza recursivamente los compuestos
/// (list/struct/map/union/array) con indentación. Los escalares usan el
/// mismo formato que `cell_value_to_string`.
pub fn cell_value_to_pretty(row: &duckdb::Row<'_>, i: usize) -> String {
    match row.get_ref(i) {
        Ok(v) => value_to_pretty(&v.to_owned(), 0),
        Err(e) => format!("<error: {e}>"),
    }
}

/// Render recursivo de un `Value` owned. `indent` es la profundidad actual;
/// los compuestos emiten `\n` + indentación por nivel.
///
/// Regla de listas/arrays (estilo numpy/pandas): el PRIMER nivel de la
/// estructura son "filas" y se ponen una por línea; todo lo que esté más
/// adentro se deja compacto en una línea. Así una matriz 2D queda:
/// `[ [11, 12], [13, 14] ]` → cada fila en su línea.
fn value_to_pretty(v: &duckdb::types::Value, indent: usize) -> String {
    use duckdb::types::Value;
    match v {
        Value::Null => "[NULL]".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(n) => n.to_string(),
        Value::SmallInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::BigInt(n) => n.to_string(),
        Value::HugeInt(n) => n.to_string(),
        Value::UHugeInt(n) => n.to_string(),
        Value::UTinyInt(n) => n.to_string(),
        Value::USmallInt(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::UBigInt(n) => n.to_string(),
        Value::Float(n) => n.to_string(),
        Value::Double(n) => n.to_string(),
        Value::Decimal(n) => n.to_string(),
        // Text que parece JSON (p.ej. payload_json) → formateado pretty
        Value::Text(t) => pretty_json_or_plain(t),
        Value::Blob(b) => format!("0x{}", hex(b)),
        Value::Geometry(b) => format!("WKB[{}B]", b.len()),
        Value::Date32(d) => format!("{y:04}-{:02}-{:02}", month(*d), day(*d), y = year(*d)),
        Value::Time64(tu, t) => time_to_string(*tu, *t),
        Value::Timestamp(tu, t) => timestamp_to_string(*tu, *t),
        Value::Interval { months, days, nanos } => interval_to_string(*months, *days, *nanos),
        Value::Enum(s) => s.clone(),
        Value::List(items) | Value::Array(items) => list_to_pretty(items, indent),
        Value::Struct(map) => named_map_to_pretty(map, indent),
        Value::Map(map) => map_to_pretty(map, indent),
        Value::Union(inner) => {
            // Escalar → inline `union(v)`; compuesto → `union` + bloque.
            if is_compound(inner) {
                format!("<union>\n{}", value_to_pretty(inner, indent + 1))
            } else {
                format!("union({})", value_compact(inner))
            }
        }
        // Value es non-exhaustive: variantes futuras
        _ => "<otro>".to_string(),
    }
}

/// ¿El valor tiene estructura interna (lista/struct/map/union)?
const fn is_compound(v: &duckdb::types::Value) -> bool {
    matches!(
        v,
        duckdb::types::Value::List(_)
            | duckdb::types::Value::Array(_)
            | duckdb::types::Value::Struct(_)
            | duckdb::types::Value::Map(_)
            | duckdb::types::Value::Union(_)
    )
}

/// Render COMPACTO (una sola línea, sin `\n`): para filas de matrices y
/// valores dentro de listas/structs que deben quedar inline.
fn value_compact(v: &duckdb::types::Value) -> String {
    use duckdb::types::Value;
    match v {
        Value::Null => "[NULL]".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(n) => n.to_string(),
        Value::SmallInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::BigInt(n) => n.to_string(),
        Value::HugeInt(n) => n.to_string(),
        Value::UHugeInt(n) => n.to_string(),
        Value::UTinyInt(n) => n.to_string(),
        Value::USmallInt(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::UBigInt(n) => n.to_string(),
        Value::Float(n) => n.to_string(),
        Value::Double(n) => n.to_string(),
        Value::Decimal(n) => n.to_string(),
        Value::Text(t) => pretty_json_or_plain(t),
        Value::Blob(b) => format!("0x{}", hex(b)),
        Value::Geometry(b) => format!("WKB[{}B]", b.len()),
        Value::Date32(d) => format!("{y:04}-{:02}-{:02}", month(*d), day(*d), y = year(*d)),
        Value::Time64(tu, t) => time_to_string(*tu, *t),
        Value::Timestamp(tu, t) => timestamp_to_string(*tu, *t),
        Value::Interval { months, days, nanos } => interval_to_string(*months, *days, *nanos),
        Value::Enum(s) => s.clone(),
        Value::List(items) | Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(value_compact).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Struct(map) => {
            let inner: Vec<String> =
                map.iter().map(|(k, val)| format!("{k}: {}", value_compact(val))).collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::Map(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{}: {}", value_compact(k), value_compact(val)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::Union(inner) => format!("union({})", value_compact(inner)),
        _ => "<otro>".to_string(),
    }
}

/// Texto que parece JSON (empieza por `{` o `[`) → pretty de `serde_json`.
/// Cualquier otro texto se devuelve tal cual.
fn pretty_json_or_plain(t: &str) -> String {
    let trimmed = t.trim();
    let looks_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    if looks_json {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                return pretty;
            }
        }
    }
    t.to_string()
}

/// Lista/Array → primer nivel en "filas" (una por línea); los elementos que
/// son a su vez compuestos se dejan compactos en su línea (numpy style).
fn list_to_pretty(items: &[duckdb::types::Value], indent: usize) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    // Todos escalares → una sola línea (p.ej. [11, 12, 13])
    if items.iter().all(|v| !is_compound(v)) {
        let inner: Vec<String> = items.iter().map(value_compact).collect();
        return format!("[{}]", inner.join(", "));
    }
    // Hay compuestos: cada elemento es una "fila" en su propia línea
    let pad = "  ".repeat(indent + 1);
    let mut out = String::from("[\n");
    for (i, item) in items.iter().enumerate() {
        out.push_str(&pad);
        // Elemento compuesto anidado → compacto (matriz K>1: lo interno sin
        // saltos); struct/map sí se expanden porque son legibles así.
        match item {
            duckdb::types::Value::List(_) | duckdb::types::Value::Array(_) => {
                out.push_str(&value_compact(item));
            }
            _ => {
                let rendered = value_to_pretty(item, indent + 1);
                // Sangrar las líneas internas para que la fila quede alineada
                let rendered = rendered.replace('\n', &format!("\n{pad}"));
                out.push_str(&rendered);
            }
        }
        if i + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&"  ".repeat(indent));
    out.push(']');
    out
}

/// Struct → `{ nombre: valor, ... }` con un campo por línea.
fn named_map_to_pretty(
    map: &duckdb::types::OrderedMap<String, duckdb::types::Value>,
    indent: usize,
) -> String {
    if map.iter().next().is_none() {
        return "{}".to_string();
    }
    let pad = "  ".repeat(indent + 1);
    let mut out = String::from("{\n");
    let mut first = true;
    for (k, v) in map.iter() {
        if !first {
            out.push(',');
            out.push('\n');
        }
        first = false;
        out.push_str(&pad);
        out.push_str(k);
        out.push_str(": ");
        out.push_str(&value_to_pretty(v, indent + 1));
    }
    out.push('\n');
    out.push_str(&"  ".repeat(indent));
    out.push('}');
    out
}

/// Map → `{ clave: valor, ... }` (claves también pueden ser compuestas).
fn map_to_pretty(
    map: &duckdb::types::OrderedMap<duckdb::types::Value, duckdb::types::Value>,
    indent: usize,
) -> String {
    if map.iter().next().is_none() {
        return "{}".to_string();
    }
    let pad = "  ".repeat(indent + 1);
    let mut out = String::from("{\n");
    let mut first = true;
    for (k, v) in map.iter() {
        if !first {
            out.push(',');
            out.push('\n');
        }
        first = false;
        out.push_str(&pad);
        out.push_str(&value_to_pretty(k, indent + 1));
        out.push_str(": ");
        out.push_str(&value_to_pretty(v, indent + 1));
    }
    out.push('\n');
    out.push_str(&"  ".repeat(indent));
    out.push('}');
    out
}

/// Bytes en hexadecimal compacto.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Días desde 1970-01-01 → mes (1-12). Algoritmo `civil_from_days` (Hinnant).
fn month(days: i32) -> u32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(0)
}

/// Días desde 1970-01-01 → día del mes (1-31).
fn day(days: i32) -> u32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    u32::try_from(d).unwrap_or(0)
}

/// Convierte el tick de Time64 (hora del día) a HH:MM:SS(.ffff).
fn time_to_string(tu: duckdb::types::TimeUnit, v: i64) -> String {
    let ticks_per_sec = match tu {
        duckdb::types::TimeUnit::Second => 1,
        duckdb::types::TimeUnit::Millisecond => 1_000,
        duckdb::types::TimeUnit::Microsecond => 1_000_000,
        duckdb::types::TimeUnit::Nanosecond => 1_000_000_000,
    };
    let total = v.div_euclid(ticks_per_sec);
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    let frac = v.rem_euclid(ticks_per_sec);
    let frac_str =
        if frac > 0 { format!(".{:06}", frac * 1_000_000 / ticks_per_sec) } else { String::new() };
    format!("{h:02}:{m:02}:{s:02}{frac_str}")
}

/// Timestamp → `YYYY-MM-DD HH:MM:SS(.ffff)` (usando el tick como UTC; la
/// fecha civil se deriva con `civil_from_days`).
fn timestamp_to_string(tu: duckdb::types::TimeUnit, v: i64) -> String {
    let ticks_per_sec = match tu {
        duckdb::types::TimeUnit::Second => 1,
        duckdb::types::TimeUnit::Millisecond => 1_000,
        duckdb::types::TimeUnit::Microsecond => 1_000_000,
        duckdb::types::TimeUnit::Nanosecond => 1_000_000_000,
    };
    let seconds = v.div_euclid(ticks_per_sec);
    #[allow(clippy::cast_possible_truncation)]
    let days = seconds.div_euclid(86_400) as i32;
    let y = year(days);
    let frac = v.rem_euclid(ticks_per_sec);
    let frac_str =
        if frac > 0 { format!(".{:06}", frac * 1_000_000 / ticks_per_sec) } else { String::new() };
    // Hora del día: se restan los días completos antes de pasar a time_to_string.
    let day_seconds = (seconds - i64::from(days) * 86_400) * ticks_per_sec;
    format!(
        "{y:04}-{:02}-{:02} {}{frac_str}",
        month(days),
        day(days),
        time_to_string(tu, day_seconds),
    )
}

/// Días desde 1970-01-01 → año.
const fn year(days: i32) -> i32 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    yoe + era * 400
}

/// Intervalo → `Xm Yd HH:MM:SS` (partes no nulas), estilo duckdb CLI.
fn interval_to_string(months: i32, days: i32, nanos: i64) -> String {
    let total_secs = nanos.div_euclid(1_000_000_000);
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    let mut parts = Vec::new();
    if months != 0 {
        parts.push(format!("{months}m"));
    }
    if days != 0 {
        parts.push(format!("{days}d"));
    }
    if h != 0 || m != 0 || s != 0 {
        parts.push(format!("{h:02}:{m:02}:{s:02}"));
    }
    if parts.is_empty() { "0".to_string() } else { parts.join(" ") }
}

/// Longitud de una lista/array de arrow en la posición `i` (número de
/// elementos de la sub-lista, no de filas del array).
fn list_len(list: duckdb::types::ListType<'_>, i: usize) -> usize {
    match list {
        duckdb::types::ListType::Regular(arr) => usize::try_from(arr.value_length(i)).unwrap_or(0),
        duckdb::types::ListType::Large(arr) => usize::try_from(arr.value_length(i)).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DB temporal con una tabla que incluye un valor con pipes dentro de
    /// una celda: el bug que motivó los modelos tipados.
    fn temp_db(name: &str) -> (std::path::PathBuf, impl FnOnce()) {
        let dir =
            std::env::temp_dir().join(format!("lazydb_test_ddb_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("model.duckdb");
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
        let dir =
            std::env::temp_dir().join(format!("lazydb_test_ddb_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("fk.duckdb");
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

    #[test]
    fn list_objects_by_type_encuentra_tablas_y_vistas() {
        let (path, cleanup) = temp_db("objects");
        let conn = Connection::open(&path).expect("abrir");
        conn.execute_batch(
            "CREATE VIEW v AS SELECT 1 AS x;
             CREATE INDEX idx_t_note ON t (note);",
        )
        .expect("schema");
        drop(conn);

        let tables = list_objects_by_type(path.to_str().unwrap(), "table").expect("tablas");
        assert!(tables.contains(&"t".to_string()));
        assert!(!tables.contains(&"v".to_string()));

        let views = list_objects_by_type(path.to_str().unwrap(), "view").expect("vistas");
        assert!(views.contains(&"v".to_string()));

        let idxs = list_objects_by_type(path.to_str().unwrap(), "index").expect("índices");
        assert!(idxs.contains(&"idx_t_note".to_string()));

        cleanup();
    }

    #[test]
    fn object_sql_reconstruye_create_table() {
        let (path, cleanup) = temp_db("ddl");
        let sql = object_sql(path.to_str().unwrap(), "t").expect("ddl");
        assert!(sql.contains("CREATE TABLE"));
        assert!(sql.contains("id"));
        assert!(sql.contains("note"));
        cleanup();
    }

    #[test]
    fn table_columns_devuelve_nulabilidad() {
        let (path, cleanup) = temp_db("cols");
        let cols = table_columns(path.to_str().unwrap(), "t").expect("columnas");
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "id");
        assert!(!cols[0].notnull);
        assert_eq!(cols[1].name, "note");
        assert!(!cols[1].notnull);
        cleanup();
    }

    #[test]
    fn render_fechas_y_horas_usa_fecha_civil() {
        use duckdb::types::TimeUnit;
        // 2026-08-03 01:13:22 (epoch seconds, verificado con date +%s)
        let epoch = 1_785_719_602_i64;
        let s = timestamp_to_string(TimeUnit::Second, epoch);
        assert_eq!(s, "2026-08-03 01:13:22", "got: {s}");

        // Fechas antes de 1970 (negativos)
        let s = timestamp_to_string(TimeUnit::Second, -1);
        assert_eq!(s, "1969-12-31 23:59:59", "got: {s}");

        // Date32: 20668 días → 2026-08-03
        let s = format!("{y:04}-{:02}-{:02}", month(20668), day(20668), y = year(20668));
        assert_eq!(s, "2026-08-03", "got: {s}");

        // Intervalo legible
        assert_eq!(interval_to_string(2, 3, 3_661_000_000_000), "2m 3d 01:01:01");
        assert_eq!(interval_to_string(0, 0, 0), "0");
    }

    #[test]
    fn render_compuestos_usa_regla_numpy() {
        use duckdb::types::Value;

        // Lista 1D de escalares → una línea (etiquetas)
        let v = Value::List(vec![
            Value::Text("dev".into()),
            Value::Text("test".into()),
            Value::Text("v1".into()),
        ]);
        assert_eq!(value_to_pretty(&v, 0), "[dev, test, v1]");

        // Matriz 2D → cada fila en su línea, elementos internos compactos
        let v = Value::List(vec![
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(3), Value::Int(4)]),
        ]);
        assert_eq!(value_to_pretty(&v, 0), "[\n  [1, 2],\n  [3, 4]\n]");

        // Matriz 3D → solo el primer nivel en líneas
        let v = Value::List(vec![
            Value::List(vec![
                Value::List(vec![Value::Int(1), Value::Int(2)]),
                Value::List(vec![Value::Int(3), Value::Int(4)]),
            ]),
            Value::List(vec![
                Value::List(vec![Value::Int(5), Value::Int(6)]),
                Value::List(vec![Value::Int(7), Value::Int(8)]),
            ]),
        ]);
        assert_eq!(value_to_pretty(&v, 0), "[\n  [[1, 2], [3, 4]],\n  [[5, 6], [7, 8]]\n]");

        // Lista vacía
        assert_eq!(value_to_pretty(&Value::List(vec![]), 0), "[]");

        // Union con escalar → inline; con compuesto → bloque
        assert_eq!(
            value_to_pretty(&Value::Union(Box::new(Value::Text("texto_7".into()))), 0),
            "union(texto_7)"
        );
    }

    #[test]
    fn render_texto_json_se_formatea_pretty() {
        // Texto que parece JSON → pretty (serde_json)
        let s = r#"{"a":1,"b":[1,2]}"#;
        assert_eq!(pretty_json_or_plain(s), "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ]\n}");

        // Texto normal → tal cual
        assert_eq!(pretty_json_or_plain("Código-1"), "Código-1");
        assert_eq!(pretty_json_or_plain("no es json {abierto"), "no es json {abierto");
    }

    /// Smoke test contra la DB de prueba real del usuario. Se ejecuta con
    /// `cargo test -- --ignored --nocapture` (requiere el archivo en disco).
    #[test]
    #[ignore = "requiere /home/cesar/dev/lazydb/fw2-aai_Latn.duckdb en disco"]
    fn smoke_db_real_usuario() {
        let path = "/home/cesar/dev/lazydb/fw2-aai_Latn.duckdb";
        let tables = list_objects_by_type(path, "table").expect("tablas");
        println!("TABLAS: {tables:?}");
        for t in &tables {
            let cols = table_columns(path, t).expect("columnas");
            println!("  {t}: {} columnas", cols.len());
            for c in &cols {
                println!("    - {} {} notnull={} pk={}", c.name, c.dtype, c.notnull, c.pk);
            }
            let n = table_row_count(path, t).expect("count");
            println!("  {t}: {n} filas");
            let data = table_rows(path, t, 3, 0).expect("rows");
            for r in &data.rows {
                println!("    row: {:?}", r.cells);
            }
            let ddl = object_sql(path, t).expect("ddl");
            println!("  DDL: {}", ddl.lines().next().unwrap_or(""));
        }
        let fks = foreign_keys(path, "data").expect("fks");
        println!("FKs de data: {fks:?}");
    }

    /// Smoke test replicando el flujo EXACTO de la UI (`normalize_path` +
    /// resolver + catálogo) sobre ambos archivos reales del usuario, con el
    /// error real visible si algo falla.
    #[test]
    #[ignore = "requiere los .duckdb del usuario en disco"]
    fn smoke_flujo_ui_completo() {
        for path in [
            "/home/cesar/dev/lazydb/fw2-aai_Latn.duckdb",
            "/home/cesar/dev/lazydb/mi_test_db.duckdb",
        ] {
            println!("=== {path} ===");
            let normalized = crate::paths::normalize_path(path);
            println!("normalize_path → {normalized}");
            let adapter = crate::db::resolver::resolve_backend(&normalized)
                .expect("resolver debe reconocer duckdb");
            match adapter.list_objects_by_type("table") {
                Ok(tables) => println!("  TABLAS: {tables:?}"),
                Err(err) => println!("  ERROR REAL: {err:?}"),
            }
            match adapter.list_objects_by_type("view") {
                Ok(views) => println!("  VISTAS: {views:?}"),
                Err(err) => println!("  ERROR REAL views: {err:?}"),
            }
            match adapter.list_objects_by_type("index") {
                Ok(idx) => println!("  ÍNDICES: {idx:?}"),
                Err(err) => println!("  ERROR REAL index: {err:?}"),
            }
            match adapter.list_objects_by_type("trigger") {
                Ok(trg) => println!("  TRIGGERS: {trg:?}"),
                Err(err) => println!("  ERROR REAL trigger: {err:?}"),
            }
            match adapter.list_advanced_objects() {
                Ok(adv) => println!("  AVANZADOS: {adv:?}"),
                Err(err) => println!("  ERROR REAL advanced: {err:?}"),
            }

            // Path exacto del inspector de fila (panic "statement not executed"):
            // table_data_rows debe funcionar sobre TODAS las tablas del archivo.
            if let Ok(tables) = adapter.list_objects_by_type("table") {
                for t in tables {
                    match crate::db::backends::duckdb::table_data_rows(&normalized, &t, 2, 0) {
                        Ok(rows) => {
                            println!("  inspector {t}: {} filas", rows.len());
                            if let Some(row) = rows.first() {
                                println!("    {row:?}");
                            }
                        }
                        Err(err) => println!("  ERROR REAL inspector {t}: {err:?}"),
                    }
                }
            }

            // Inspector expandido: celdas multilínea con compuestos completos.
            if let Ok(tables) = adapter.list_objects_by_type("table") {
                for t in tables {
                    match crate::db::backends::duckdb::table_data_rows_pretty(&normalized, &t, 1, 0)
                    {
                        Ok(rows) => {
                            if let Some(row) = rows.first() {
                                for (ci, cell) in row.cells.iter().enumerate() {
                                    if cell.contains('\n') {
                                        println!("  EXPANDIDO {t} col{ci}:");
                                        for line in cell.split('\n') {
                                            println!("    | {line}");
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => println!("  ERROR REAL expandido {t}: {err:?}"),
                    }
                }
            }
        }
    }

    /// Smoke de archivos de datos locales (filosofía lazy: parquet/csv/json/
    /// geojson/gpkg). Requiere los archivos de prueba en /tmp/opencode
    /// (generados con la CLI duckdb) y red la primera vez para `spatial`.
    /// Se ejecuta con `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "requiere archivos de prueba en /tmp/opencode + red para spatial"]
    fn smoke_archivos_de_datos() {
        let base = "/tmp/opencode";
        for file in ["datos.parquet", "datos.csv", "datos.json", "lugares.geojson", "ciudades.gpkg"]
        {
            let path = format!("{base}/{file}");
            let dataset = crate::db::backends::file::dataset_name(&path);
            println!("=== {path} ===");
            let adapter = crate::db::resolver::resolve_backend(&path)
                .expect("resolver debe reconocer el archivo");
            match adapter.list_objects_by_type("table") {
                Ok(tables) => println!("  TABLAS: {tables:?}"),
                Err(err) => {
                    println!("  ERROR REAL: {err:?}");
                    continue;
                }
            }
            match adapter.table_row_count(&dataset) {
                Ok(n) => println!("  COUNT: {n}"),
                Err(err) => println!("  ERROR COUNT: {err:?}"),
            }
            match adapter.column_names(&dataset) {
                Ok(cols) => println!("  COLUMNAS: {cols:?}"),
                Err(err) => println!("  ERROR COLUMNAS: {err:?}"),
            }
            match adapter.table_rows(&dataset, 2, 0) {
                Ok(data) => {
                    for row in &data.rows {
                        println!("    row: {row:?}");
                    }
                }
                Err(err) => println!("  ERROR ROWS: {err:?}"),
            }
            match adapter.object_sql(&dataset) {
                Ok(sql) => println!("  DDL: {sql}"),
                Err(err) => println!("  ERROR DDL: {err:?}"),
            }
            match crate::db::backends::file::table_data_rows_pretty(&path, 1, 0) {
                Ok(rows) => {
                    if let Some(row) = rows.first() {
                        println!("  INSPECTOR: {row:?}");
                    }
                }
                Err(err) => println!("  ERROR INSPECTOR: {err:?}"),
            }
            // Query libre (pop-up de SQL) contra el dataset virtual.
            match crate::db::backends::file::query_free(
                &path,
                &format!("SELECT count(*) AS n FROM \"{dataset}\""),
                5,
            ) {
                Ok(rows) => println!("  QUERY LIBRE: {rows:?}"),
                Err(err) => println!("  ERROR QUERY LIBRE: {err:?}"),
            }
        }
    }

    /// Smoke de MySQL/MariaDB localhost: requiere la URL en el env
    /// `LAZYDB_MYSQL_URL` (ej. `mysql://lazydb:lazydb123@127.0.0.1:3306/lazydb_demo`).
    /// Creditos: no hardcodear credenciales en el repo → env var + `#[ignore]`.
    ///
    /// ```sql
    /// CREATE USER IF NOT EXISTS 'lazydb'@'localhost' IDENTIFIED BY 'lazydb123';
    /// GRANT ALL PRIVILEGES ON lazydb_demo.* TO 'lazydb'@'localhost';
    /// FLUSH PRIVILEGES;
    /// ```
    #[test]
    #[ignore = "requiere MariaDB local + LAZYDB_MYSQL_URL"]
    fn smoke_mysql_localhost() {
        let Some(url) = std::env::var("LAZYDB_MYSQL_URL").ok() else {
            eprintln!("    ⚠︎ LAZYDB_MYSQL_URL no definida — omitiendo smoke MySQL");
            return;
        };
        println!("=== {url} (host y user ocultos por privacidad) ===");
        let adapter =
            crate::db::resolver::resolve_backend(&url).expect("resolver debe reconocer mysql://");

        match adapter.list_objects_by_type("table") {
            Ok(tables) => println!("  TABLAS: {tables:?}"),
            Err(err) => {
                println!("  ERROR REAL: {err:?}");
                return;
            }
        }
        match adapter.list_objects_by_type("view") {
            Ok(views) => println!("  VISTAS: {views:?}"),
            Err(err) => println!("  ERROR VISTAS: {err:?}"),
        }
        match adapter.list_advanced_objects() {
            Ok(adv) => println!("  AVANZADOS (índices+triggers): {adv:?}"),
            Err(err) => println!("  ERROR AVANZADOS: {err:?}"),
        }
        match adapter.table_row_count("categories") {
            Ok(n) => println!("  COUNT categories: {n}"),
            Err(err) => println!("  ERROR COUNT: {err:?}"),
        }
        match adapter.column_names("categories") {
            Ok(cols) => println!("  COLUMNAS categories: {cols:?}"),
            Err(err) => println!("  ERROR COLUMNAS: {err:?}"),
        }
        match adapter.table_rows("categories", 3, 0) {
            Ok(data) => {
                for row in &data.rows {
                    println!("    row: {row:?}");
                }
            }
            Err(err) => println!("  ERROR ROWS: {err:?}"),
        }
        match adapter.foreign_keys("categories") {
            Ok(fks) => println!("  FKs categories: {fks:?}"),
            Err(err) => println!("  ERROR FK: {err:?}"),
        }
        // order_items tiene FKs reales (fk_items_orders, fk_items_products)
        match adapter.foreign_keys("order_items") {
            Ok(fks) => println!("  FKs order_items: {fks:?}"),
            Err(err) => println!("  ERROR FKs order_items: {err:?}"),
        }
        // FK jump: offset de una row por valor de columna
        match adapter.row_offset_of("products", "id", "2") {
            Ok(off) => println!("  OFFSET products id=2: {off:?}"),
            Err(err) => println!("  ERROR OFFSET: {err:?}"),
        }
        match adapter.table_columns("categories") {
            Ok(info) => {
                println!("  SCHEMA categories:");
                for c in &info {
                    println!("    {c:?}");
                }
            }
            Err(err) => println!("  ERROR SCHEMA: {err:?}"),
        }
        match adapter.object_sql("categories") {
            Ok(ddl) => println!("  DDL categories: {}", ddl.lines().next().unwrap_or("")),
            Err(err) => println!("  ERROR DDL: {err:?}"),
        }
        match adapter.object_sql("categories") {
            Ok(ddl) => println!("  DDL categories: {}", ddl.lines().next().unwrap_or("")),
            Err(err) => println!("  ERROR DDL: {err:?}"),
        }
        match adapter.query("SELECT id, name FROM categories ORDER BY id", 5) {
            Ok(rows) => println!("  QUERY LIBRE: {rows:?}"),
            Err(err) => println!("  ERROR QUERY: {err:?}"),
        }

        // Conexión a nivel de SERVIDOR (sin BD): listar bases disponibles.
        // Quitamos la BD de la URL (`.../lazydb_demo` → `.../`) para conectar
        // solo al host y hacer SHOW DATABASES.
        let trimmed = url.trim_end_matches('/');
        let server_url = trimmed
            .rfind('/')
            .map_or_else(|| trimmed.to_string(), |idx| trimmed[..=idx].to_string());
        match crate::db::backends::mysql::connect(&server_url) {
            Ok((pool, db_name)) => {
                println!("  SERVER connect db_name='{db_name}'");
                match crate::db::backends::mysql::list_databases(&pool) {
                    Ok(dbs) => println!("  BASES: {dbs:?}"),
                    Err(err) => println!("  ERROR BASES: {err:?}"),
                }
                let _ = crate::db::backends::mysql::block_on(pool.disconnect());
            }
            Err(err) => println!("  ERROR SERVER CONNECT: {err:?}"),
        }
    }
}
