// Driver de archivos de datos locales (filosofía lazy: "haz visible TODA tu
// infraestructura de datos local sin configuración").
//
// Un archivo de datos (.csv/.tsv/.parquet/.json/.jsonl/.geojson/.gpkg) se
// expone como un DATASET VIRTUAL: una conexión DuckDB en memoria con una
// vista `CREATE VIEW <nombre> AS SELECT * FROM read_parquet('ruta')`, etc.
// Todo el pipeline existente (columnas, filas, count, sorted, inspector)
// funciona sin cambios: el contrato `DbAdapter` no sabe que detrás hay un
// archivo plano.
//
// - CSV/TSV/Parquet/JSON → lectores nativos de DuckDB (sin extensiones)
// - GeoJSON/GeoPackage → extensión `spatial` (`st_read`); se instala/carga
//   bajo demanda (requiere red la primera vez)
use std::path::Path;

use duckdb::Connection;

use crate::db::{Column, ColumnInfo, DbError, Row, TableData};

/// Tipos de archivo de datos soportados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Csv,
    Tsv,
    Parquet,
    Json,
    Jsonl,
    GeoJson,
    GeoPackage,
}

/// ¿El path corresponde a un archivo de datos soportado?
pub fn kind_for(path: &str) -> Option<FileKind> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "csv" => Some(FileKind::Csv),
        "tsv" => Some(FileKind::Tsv),
        "parquet" | "pq" => Some(FileKind::Parquet),
        "json" => Some(FileKind::Json),
        "jsonl" | "ndjson" => Some(FileKind::Jsonl),
        "geojson" => Some(FileKind::GeoJson),
        "gpkg" => Some(FileKind::GeoPackage),
        _ => None,
    }
}

/// Nombre del dataset virtual = nombre del archivo sin extensión
/// (`datos.parquet` → `datos`). Es la única "tabla" que expone el archivo.
pub fn dataset_name(path: &str) -> String {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("data").to_string()
}

/// Expresión de lectura que `DuckDB` usa para el archivo.
fn read_expr(kind: FileKind, path: &str) -> String {
    // Escape de comillas simples (SQL string literal)
    let p = path.replace('\'', "''");
    match kind {
        FileKind::Csv => format!("read_csv_auto('{p}')"),
        FileKind::Tsv => format!("read_csv_auto('{p}', delim='\\t')"),
        FileKind::Parquet => format!("read_parquet('{p}')"),
        FileKind::Json => format!("read_json_auto('{p}')"),
        FileKind::Jsonl => format!("read_json_auto('{p}', format='newline_delimited')"),
        FileKind::GeoJson | FileKind::GeoPackage => format!("st_read('{p}')"),
    }
}

/// Abre una conexión `DuckDB` en memoria con el archivo registrado como vista.
/// Cada llamada re-abre (patrón de los otros drivers: funciones puras
/// path → Result); la vista es lazy, no se materializa nada.
pub fn open_dataset(path: &str) -> Result<Connection, DbError> {
    let Some(kind) = kind_for(path) else {
        return Err(DbError::Open(format!(
            "{path}: extensión no soportada (csv/tsv/parquet/json/jsonl/geojson/gpkg)"
        )));
    };
    let conn = Connection::open_in_memory()?;
    if matches!(kind, FileKind::GeoJson | FileKind::GeoPackage) {
        conn.execute_batch("INSTALL spatial; LOAD spatial;").map_err(|e| {
            DbError::Open(format!(
                "{path}: la extensión espacial de DuckDB no pudo cargarse (¿sin red la \
                 primera vez?). {e}"
            ))
        })?;
    }
    let name = dataset_name(path).replace('"', "\"\"");
    let sql = format!("CREATE VIEW \"{name}\" AS SELECT * FROM {}", read_expr(kind, path));
    conn.execute_batch(&sql)?;
    Ok(conn)
}

/// La "tabla" del archivo (dataset virtual único).
pub fn list_tables(path: &str) -> Result<Vec<String>, DbError> {
    open_dataset(path)?; // valida que el archivo se pueda abrir
    Ok(vec![dataset_name(path)])
}

/// Columnas del dataset (mismo orden que `SELECT *`), vía catálogo.
pub fn column_names(path: &str) -> Result<Vec<Column>, DbError> {
    let conn = open_dataset(path)?;
    column_names_conn(&conn, &dataset_name(path))
}

/// Igual que `column_names` pero sobre la conexión ya abierta.
/// IMPORTANTE: usar la MISMA conexión que luego ejecuta el SELECT del
/// dataset (con `spatial` cargada, abrir una segunda conexión concurrente
/// provocaba `TransactionContext::ActiveTransaction called without active
/// transaction` al consultar un geojson).
fn column_names_conn(conn: &Connection, table: &str) -> Result<Vec<Column>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT column_name, data_type FROM information_schema.columns
         WHERE table_name = ?1 AND table_schema = 'main'
         ORDER BY ordinal_position",
    )?;
    let rows =
        stmt.query_map([table], |row| Ok(Column { name: row.get(0)?, dtype: row.get(1)? }))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Columnas del dataset con su tipo real (vía `information_schema`). Distinto
/// de `column_names_conn` solo en que conservamos el dtype crudo para
/// detectar columnas GEOMETRY.
#[allow(dead_code)]
fn dataset_columns(path: &str) -> Result<Vec<Column>, DbError> {
    let conn = open_dataset(path)?;
    column_names_conn(&conn, &dataset_name(path))
}

/// Proyección `SELECT` sobre el dataset: `ST_AsText("col") AS "col"` para
/// columnas `GEOMETRY`. La extensión `spatial` de `DuckDB` rompe la transacción
/// (`ActiveTransaction called without active transaction`) al materializar el
/// binario WKB de una geometría; convertirlas a texto la evita y además hace
/// la celda legible en la TUI.
fn select_projection(columns: &[Column]) -> String {
    if columns.is_empty() {
        return "*".to_string();
    }
    let mut parts = Vec::with_capacity(columns.len());
    for col in columns {
        let name = col.name.replace('"', "\"\"");
        let is_geom = col.dtype.to_ascii_lowercase().starts_with("geometry")
            || col.dtype.to_ascii_lowercase().starts_with("geography")
            || col.dtype.to_ascii_lowercase().starts_with("bblob");
        if is_geom {
            parts.push(format!("ST_AsText(\"{name}\") AS \"{name}\""));
        } else {
            parts.push(format!("\"{name}\""));
        }
    }
    parts.join(", ")
}

/// Metadata de columnas (`PRAGMA table_info`, igual que duckdb.rs).
pub fn table_columns(path: &str) -> Result<Vec<ColumnInfo>, DbError> {
    let conn = open_dataset(path)?;
    let table = dataset_name(path).replace('\'', "''");
    let sql = format!("PRAGMA table_info('{table}')");
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

/// Filas de datos SIN header (inspector). Celdas renderizadas con el mismo
/// render que duckdb (tipos avanzados, compuestos expandidos en `pretty`).
pub fn table_data_rows(path: &str, limit: u32, offset: u32) -> Result<Vec<Row>, DbError> {
    rows_impl(path, limit, offset, false)
}

pub fn table_data_rows_pretty(path: &str, limit: u32, offset: u32) -> Result<Vec<Row>, DbError> {
    rows_impl(path, limit, offset, true)
}

fn rows_impl(path: &str, limit: u32, offset: u32, pretty: bool) -> Result<Vec<Row>, DbError> {
    let conn = open_dataset(path)?;
    let table = dataset_name(path).replace('"', "\"\"");
    // Columnas + proyección ANTES de preparar el SELECT (misma conexión):
    // con `spatial`, el SELECT * con GEOMETRY rompe la transacción.
    let columns = column_names_conn(&conn, &dataset_name(path))?;
    let projection = select_projection(&columns);
    let sql = format!("SELECT {projection} FROM \"{table}\" LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql)?;

    let mut rows = stmt.query([])?;
    let col_count = rows.as_ref().expect("stmt").column_count();

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() >= limit as usize {
            break;
        }
        let mut values = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let cell = if pretty {
                crate::db::backends::duckdb::cell_value_to_pretty(row, i)
            } else {
                crate::db::backends::duckdb::cell_value_to_string(row, i)
            };
            values.push(cell);
        }
        out.push(Row { cells: values });
    }
    Ok(out)
}

/// Filas + columnas (preview del Data tab), con ORDER BY opcional.
pub fn table_rows_sorted(
    path: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>,
) -> Result<TableData, DbError> {
    let conn = open_dataset(path)?;
    let table = dataset_name(path).replace('"', "\"\"");
    // Columnas + proyección ANTES de preparar el SELECT (misma conexión):
    // con `spatial`, el SELECT * con GEOMETRY rompe la transacción.
    let columns = column_names_conn(&conn, &dataset_name(path))?;
    let projection = select_projection(&columns);
    // ORDER BY sobre la columna ORIGINAL (no sobre ST_AsText, que duplicaría
    // la columna en el output).
    let order_clause = if let Some((col, asc)) = order_col {
        let col_esc = col.replace('"', "\"\"");
        let dir = if asc { "ASC" } else { "DESC" };
        format!(" ORDER BY \"{col_esc}\" {dir}")
    } else {
        String::new()
    };
    let sql =
        format!("SELECT {projection} FROM \"{table}\"{order_clause} LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql)?;

    let mut rows = stmt.query([])?;
    let col_count = rows.as_ref().expect("stmt").column_count();

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if out.len() >= limit as usize {
            break;
        }
        let mut values = Vec::with_capacity(col_count);
        for i in 0..col_count {
            values.push(crate::db::backends::duckdb::cell_value_to_string(row, i));
        }
        out.push(Row { cells: values });
    }
    Ok(TableData { columns, rows: out })
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn table_row_count(path: &str) -> Result<u32, DbError> {
    let conn = open_dataset(path)?;
    let table = dataset_name(path).replace('"', "\"\"");
    let sql = format!("SELECT COUNT(*) FROM \"{table}\"");
    let mut stmt = conn.prepare(&sql)?;
    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    Ok(count.max(0) as u32)
}

/// DDL del dataset virtual: la definición real de la vista.
pub fn object_sql(path: &str) -> Result<String, DbError> {
    let Some(kind) = kind_for(path) else {
        return Err(DbError::Open(format!("{path}: extensión no soportada")));
    };
    let name = dataset_name(path).replace('"', "\"\"");
    Ok(format!("CREATE VIEW \"{name}\" AS SELECT * FROM {}", read_expr(kind, path)))
}

/// SQL libre del usuario (modal `:`): la vista `data` existe en la conexión.
pub fn query_free(path: &str, sql: &str, limit: u32) -> Result<Vec<String>, DbError> {
    let conn = open_dataset(path)?;
    let mut stmt = conn.prepare(sql)?;

    // duckdb-rs: column_count() panica si la query no se ejecutó → primero
    // query() y luego pedir el count vía rows.as_ref() (mismo truco que
    // duckdb.rs para no romper el borrow del stmt).
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
            row_str.push_str(&crate::db::backends::duckdb::cell_value_to_string(row, i));
        }
        out.push(row_str);
    }
    Ok(out)
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn count_free(path: &str, sql: &str) -> Result<u32, DbError> {
    let conn = open_dataset(path)?;
    let mut stmt = conn.prepare(sql)?;
    let count: i64 = stmt.query_row([], |row| row.get(0))?;
    Ok(count.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regresión de `ActiveTransaction called without active transaction`:
    /// leer filas de un dataset espacial (geojson/gpkg) rompía la transacción
    /// de `DuckDB` al materializar la columna `GEOMETRY`. La proyección con
    /// `ST_AsText` evita el bug y hace la celda legible (WKT).
    #[test]
    #[ignore = "requiere archivos de ejemplo en el repo"]
    fn filas_de_geojson_no_provocan_active_transaction() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/sample-featurecollection.geojson");
        if !std::path::Path::new(p).exists() {
            return;
        }
        let data = table_rows_sorted(p, 3, 0, None).expect("leer filas del geojson");
        assert!(!data.rows.is_empty(), "el geojson debe tener filas");
        // La columna geometry viaja como texto (WKT), no como binario roto.
        let geom_idx = data.columns.iter().position(|c| c.name.eq_ignore_ascii_case("geom"));
        if let Some(idx) = geom_idx {
            for row in &data.rows {
                if let Some(cell) = row.cells.get(idx) {
                    assert!(
                        cell.starts_with("POINT")
                            || cell.starts_with("POLYGON")
                            || cell.starts_with("LINESTRING")
                            || cell.starts_with("MULTI")
                            || cell.starts_with("GEOMETRY")
                            || cell.is_empty()
                            || cell == "NULL",
                        "celda geométrica debe ser WKT, got: {cell:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn select_projection_envuelve_geometry_con_st_astext() {
        let cols = vec![
            Column { name: "id".into(), dtype: "INTEGER".into() },
            Column { name: "geom".into(), dtype: "GEOMETRY('EPSG:4326')".into() },
            Column { name: "nombre".into(), dtype: "VARCHAR".into() },
        ];
        let sql = select_projection(&cols);
        assert!(sql.contains("ST_AsText(\"geom\")"), "got: {sql}");
        assert!(sql.contains("\"id\""), "got: {sql}");
        assert!(sql.contains("\"nombre\""), "got: {sql}");
        // Sin columnas → SELECT * (fallback)
        assert_eq!(select_projection(&[]), "*");
    }

    #[test]
    fn kind_y_nombre_se_deducen_de_la_extension() {
        assert_eq!(kind_for("/tmp/a.csv"), Some(FileKind::Csv));
        assert_eq!(kind_for("/tmp/a.tsv"), Some(FileKind::Tsv));
        assert_eq!(kind_for("/tmp/a.parquet"), Some(FileKind::Parquet));
        assert_eq!(kind_for("a.PARQUET"), Some(FileKind::Parquet));
        assert_eq!(kind_for("/tmp/a.json"), Some(FileKind::Json));
        assert_eq!(kind_for("/tmp/a.jsonl"), Some(FileKind::Jsonl));
        assert_eq!(kind_for("/tmp/a.ndjson"), Some(FileKind::Jsonl));
        assert_eq!(kind_for("/tmp/a.geojson"), Some(FileKind::GeoJson));
        assert_eq!(kind_for("/tmp/a.gpkg"), Some(FileKind::GeoPackage));
        assert_eq!(kind_for("/tmp/a.db"), None, "los .db siguen siendo sqlite");
        assert_eq!(kind_for("/tmp/a.duckdb"), None, "los .duckdb siguen siendo duckdb");

        assert_eq!(dataset_name("/tmp/datos.parquet"), "datos");
        assert_eq!(dataset_name("mi archivo.csv"), "mi archivo");
        assert_eq!(dataset_name("sin_extension"), "sin_extension");
    }

    #[test]
    fn object_sql_muestra_la_vista_virtual() {
        let sql = object_sql("/tmp/datos.csv").unwrap();
        assert_eq!(sql, "CREATE VIEW \"datos\" AS SELECT * FROM read_csv_auto('/tmp/datos.csv')");
        let sql = object_sql("/tmp/lugares.geojson").unwrap();
        assert!(sql.contains("st_read('/tmp/lugares.geojson')"), "got: {sql}");
    }

    /// El dataset virtual funciona end-to-end sobre un CSV temporal
    /// (sin red, sin extensiones): abrir, contar, columnas y filas.
    #[test]
    fn dataset_virtual_lee_un_csv_temporal() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("lazydb_test_{}.csv", std::process::id()));
        std::fs::write(&path, "id,nombre,valor\n1,alpha,10.5\n2,beta,20.25\n3,gamma,30.5\n")
            .unwrap();
        let p = path.to_str().unwrap();

        assert_eq!(table_row_count(p).unwrap(), 3);
        let cols = column_names(p).unwrap();
        assert_eq!(
            cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["id", "nombre", "valor"]
        );
        let rows = table_data_rows(p, 2, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells, ["1", "alpha", "10.5"]);
        let pretty = table_data_rows_pretty(p, 1, 2).unwrap();
        assert_eq!(pretty[0].cells, ["3", "gamma", "30.5"]);
        // ORDER BY descendente
        let sorted = table_rows_sorted(p, 10, 0, Some(("id", false))).unwrap();
        assert_eq!(sorted.rows[0].cells[0], "3");

        let _ = std::fs::remove_file(&path);
    }
}
