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
    let conn = open_read_only(path)?;
    let escaped = table_name.replace('"', "\"\"");
    let sql = format!("SELECT * FROM \"{escaped}\" LIMIT {limit} OFFSET {offset}");
    let mut stmt = conn.prepare(&sql)?;

    let col_count = stmt.column_count();

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

    // duckdb-rs: column_names() panica si la query no se ejecutó aún.
    // Obtenemos los nombres vía el catálogo (mismo orden que SELECT *).
    let columns = column_names(path, table_name)?;
    if columns.is_empty() {
        return Ok(TableData { columns, rows: Vec::new() });
    }

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..columns.len() {
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
        Ok(ValueRef::Blob(_)) => "<blob>".to_string(),
        Ok(ValueRef::Date32(_)) => "<date>".to_string(),
        Ok(ValueRef::Time64(..)) => "<time>".to_string(),
        Ok(ValueRef::Timestamp(..)) => "<timestamp>".to_string(),
        Ok(ValueRef::Interval { .. }) => "<interval>".to_string(),
        Ok(ValueRef::List(..)) => "<list>".to_string(),
        Ok(ValueRef::Enum(..)) => "<enum>".to_string(),
        Ok(ValueRef::Struct(..)) => "<struct>".to_string(),
        Ok(ValueRef::Map(..)) => "<map>".to_string(),
        Ok(ValueRef::Union(..)) => "<union>".to_string(),
        Ok(ValueRef::Array(..)) => "<array>".to_string(),
        // ValueRef es non-exhaustive en duckdb 1.10505: variantes futuras
        Ok(_) => "<otro>".to_string(),
        Err(e) => format!("<error: {e}>"),
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
        }
    }
}
