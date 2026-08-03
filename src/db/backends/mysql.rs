//! Backend MySQL/MariaDB (protocolo binario nativo vía `mysql_async`).
//!
//! Todas las funciones reciben `&Pool` + `db_name` porque la pool es de
//! larga duración (TCP) y se comparte entre todas las queries de una misma
//! fuente. El adapter (`mysql_adapter.rs`) mantiene la pool lazy-init.

use crate::db::rt::block_on;
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row, TableData};
use mysql_async::prelude::*;
use mysql_async::{Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, Row as MysqlRow, Value};

// ── Conexión ──────────────────────────────────────────────────────────

/// Crea una pool conectada a `url` (formato `mysql://user:pass@host:port/db`)
/// y devuelve la pool + nombre de la base de datos parseado.
pub fn connect(url: &str) -> Result<(Pool, String), DbError> {
    let opts = Opts::from_url(url).map_err(|e| DbError::Open(format!("URL inválida: {e}")))?;
    // La BD es OPCIONAL: si falta, la conexión apunta al servidor (útil para
    // listar bases o conectar con la BD por defecto del user).
    let db_name = opts.db_name().map(ToString::to_string).unwrap_or_default();
    let conn_limit = PoolConstraints::new(2, 5)
        .ok_or_else(|| DbError::Open("pool constraints inválidos".into()))?;
    let pool_opts = PoolOpts::default().with_constraints(conn_limit);
    let pool = Pool::new(OptsBuilder::from_opts(opts).pool_opts(pool_opts));
    Ok((pool, db_name))
}

/// Lista las bases de datos del servidor (`SHOW DATABASES`), excluyendo las
/// de sistema (`information_schema`, `mysql`, `performance_schema`, `sys`).
pub fn list_databases(pool: &Pool) -> Result<Vec<String>, DbError> {
    block_on(async {
        let mut conn = pool.get_conn().await?;
        let dbs: Vec<String> = conn.query("SHOW DATABASES").await?;
        drop(conn);
        Ok(dbs
            .into_iter()
            .filter(|d| {
                !matches!(d.as_str(), "information_schema" | "mysql" | "performance_schema" | "sys")
            })
            .collect())
    })
}

// ─── Catálogo (tablas, vistas, índices, triggers) ─────────────────────

async fn list_tables_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SHOW FULL TABLES FROM `{db_name}` WHERE Table_type = 'BASE TABLE'");
    let tables: Vec<(String, String)> = conn.query(sql).await?;
    Ok(tables.into_iter().map(|(name, _typ)| name).collect())
}

async fn list_views_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SHOW FULL TABLES FROM `{db_name}` WHERE Table_type = 'VIEW'");
    let views: Vec<(String, String)> = conn.query(sql).await?;
    Ok(views.into_iter().map(|(name, _typ)| name).collect())
}

async fn list_triggers_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!(
        "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = '{db_name}'"
    );
    let trg: Vec<(String,)> = conn.query(sql).await?;
    Ok(trg.into_iter().map(|(name,)| name).collect())
}

// ─── Metadata de columnas ─────────────────────────────────────────────

async fn column_info_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ColumnInfo>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!(
        "SELECT ORDINAL_POSITION, COLUMN_NAME, COLUMN_TYPE,
                IS_NULLABLE, COLUMN_KEY
         FROM information_schema.COLUMNS
         WHERE TABLE_SCHEMA = '{db_name}' AND TABLE_NAME = '{table_name}'
         ORDER BY ORDINAL_POSITION"
    );
    let rows: Vec<(i64, String, String, String, String)> = conn.query(sql).await?;
    let out = rows
        .into_iter()
        .map(|(pos, name, dtype, nullable, key)| ColumnInfo {
            cid: pos - 1,
            name,
            dtype,
            notnull: nullable.eq_ignore_ascii_case("NO"),
            pk: key.eq_ignore_ascii_case("PRI"),
        })
        .collect();
    Ok(out)
}

// ─── Filas (inspector, Data tab, query libre) ─────────────────────────

/// Render de un `Value` de `MySQL` a `String`.
/// El protocolo binario envía casi todo como Bytes; parseamos tipos.
fn value_to_string(v: &mysql_async::Value) -> String {
    match v {
        Value::NULL => "NULL".to_string(),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Double(d) => format!("{d}"),
        Value::Date(y, m, d, hh, mm, ss, _us) => {
            format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
        }
        Value::Time(neg, days, hh, mm, ss, _us) => {
            let sign = if *neg { "-" } else { "" };
            format!("{sign}{days}d {hh:02}:{mm:02}:{ss:02}")
        }
    }
}

async fn rows_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SELECT * FROM `{db_name}`.`{table_name}` LIMIT {limit} OFFSET {offset}");
    let rows: Vec<MysqlRow> = conn.query(sql).await?;
    let out: Vec<Row> = rows
        .into_iter()
        .map(|row| {
            let cells: Vec<String> = (0..row.len())
                .map(|i| row.as_ref(i).map_or_else(|| "NULL".into(), value_to_string))
                .collect();
            drop(row);
            Row { cells }
        })
        .collect();
    Ok(out)
}

async fn table_rows_sorted_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>,
) -> Result<TableData, DbError> {
    let order_clause = if let Some((col, asc)) = order_col {
        let dir = if asc { "ASC" } else { "DESC" };
        format!(" ORDER BY `{col}` {dir}")
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT * FROM `{db_name}`.`{table_name}`{order_clause} LIMIT {limit} OFFSET {offset}"
    );

    let columns = column_names_async(pool, db_name, table_name).await?;

    let mut conn = pool.get_conn().await?;
    let rows: Vec<MysqlRow> = conn.query(&sql).await?;

    let data_rows: Vec<Row> = rows
        .into_iter()
        .map(|row| {
            let cells: Vec<String> = (0..row.len())
                .map(|i| row.as_ref(i).map_or_else(|| "NULL".into(), value_to_string))
                .collect();
            drop(row);
            Row { cells }
        })
        .collect();
    Ok(TableData { columns, rows: data_rows })
}

async fn row_count_async(pool: &Pool, db_name: &str, table_name: &str) -> Result<u32, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SELECT COUNT(*) FROM `{db_name}`.`{table_name}`");
    let count: Option<i64> = conn.query_first(sql).await?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count.unwrap_or(0).max(0) as u32)
}

async fn column_names_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<Column>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!(
        "SELECT COLUMN_NAME, DATA_TYPE
         FROM information_schema.COLUMNS
         WHERE TABLE_SCHEMA = '{db_name}' AND TABLE_NAME = '{table_name}'
         ORDER BY ORDINAL_POSITION"
    );
    let pairs: Vec<(String, String)> = conn.query(sql).await?;
    Ok(pairs.into_iter().map(|(name, dtype)| Column { name, dtype }).collect())
}

async fn fk_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ForeignKey>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!(
        "SELECT COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME,
                ORDINAL_POSITION, CONSTRAINT_NAME
         FROM information_schema.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = '{db_name}' AND TABLE_NAME = '{table_name}'
               AND REFERENCED_TABLE_NAME IS NOT NULL
         ORDER BY ORDINAL_POSITION"
    );
    let rows: Vec<(String, String, String, i64, String)> = conn.query(sql).await?;
    Ok(rows
        .into_iter()
        .map(|(from, to_table, to_col, pos, _constraint)| ForeignKey {
            id: 0, // el ID histórico era de sqlite; mysql usa nombres
            seq: pos,
            table: to_table,
            from,
            to: if to_col.is_empty() { None } else { Some(to_col) },
        })
        .collect())
}

async fn row_offset_of_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    col: &str,
    value: &str,
) -> Result<Option<u32>, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SELECT COUNT(*) FROM `{db_name}`.`{table_name}` WHERE `{col}` < '{value}'");
    let count: Option<i64> = conn.query_first(sql).await?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count.and_then(|c| if c > 0 { Some(c as u32 - 1) } else { None }))
}

async fn object_sql_async(
    pool: &Pool,
    db_name: &str,
    object_name: &str,
) -> Result<String, DbError> {
    let mut conn = pool.get_conn().await?;
    let sql = format!("SHOW CREATE TABLE `{db_name}`.`{object_name}`");
    let rows: Vec<(String, String)> = conn.query(sql).await?;
    rows.into_iter()
        .next()
        .map(|(_name, ddl)| ddl)
        .ok_or_else(|| DbError::Open(format!("{object_name}: no encontrado")))
}

async fn query_free_async(
    pool: &Pool,
    _db_name: &str,
    sql: &str,
    limit: u32,
) -> Result<Vec<String>, DbError> {
    let mut conn = pool.get_conn().await?;
    // Limitar con LIMIT embed (el motor lo optimiza)
    let row_sql = format!("{sql} LIMIT {limit}");
    let rows: Vec<MysqlRow> = conn.query(&row_sql).await?;
    let mut out = Vec::new();
    for row in rows {
        let line = (0..row.len())
            .map(|i| row.as_ref(i).map_or_else(|| "NULL".into(), value_to_string))
            .collect::<Vec<_>>()
            .join(" | ");
        out.push(line);
    }
    Ok(out)
}

async fn count_free_async(pool: &Pool, sql: &str) -> Result<u32, DbError> {
    let mut conn = pool.get_conn().await?;
    let count: Option<i64> = conn.query_first(sql).await?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count.unwrap_or(0) as u32)
}

// ─── Wrappers síncronos (el trait NO es async) ────────────────────────
//
// El trait `DbAdapter` es síncrono, pero `mysql_async` es async. Cada
// función pública bloquea con `block_on` sobre un runtime compartido
// (`RUNTIME`, lazy-init). Como el adapter se invoca desde el event loop
// síncrono (o desde `spawn_blocking`), `block_on` no rewind la UI.

pub fn list_tables(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_tables_async(pool, db_name))
}

pub fn list_views(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_views_async(pool, db_name))
}

/// Todos los índices de la base (no solo de una tabla). El adapter los usa
/// para el panel "Indexes".
pub fn list_all_indexes(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(async {
        let mut conn = pool.get_conn().await?;
        // `statistics` contiene una fila por columna por índice → DISTINCT
        let sql = format!(
            "SELECT DISTINCT INDEX_NAME FROM information_schema.STATISTICS
             WHERE TABLE_SCHEMA = '{db_name}' AND INDEX_NAME != 'PRIMARY'
             ORDER BY INDEX_NAME"
        );
        let rows: Vec<String> = conn.query(sql).await?;
        drop(conn);
        Ok(rows)
    })
}

pub fn list_triggers(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_triggers_async(pool, db_name))
}

pub fn column_info(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ColumnInfo>, DbError> {
    block_on(column_info_async(pool, db_name, table_name))
}

pub fn table_data_rows(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    block_on(rows_async(pool, db_name, table_name, limit, offset))
}

pub fn table_rows_sorted(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>,
) -> Result<TableData, DbError> {
    block_on(table_rows_sorted_async(pool, db_name, table_name, limit, offset, order_col))
}

pub fn table_row_count(pool: &Pool, db_name: &str, table_name: &str) -> Result<u32, DbError> {
    block_on(row_count_async(pool, db_name, table_name))
}

pub fn column_names(pool: &Pool, db_name: &str, table_name: &str) -> Result<Vec<Column>, DbError> {
    block_on(column_names_async(pool, db_name, table_name))
}

pub fn foreign_keys(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ForeignKey>, DbError> {
    block_on(fk_async(pool, db_name, table_name))
}

pub fn row_offset_of(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    col: &str,
    value: &str,
) -> Result<Option<u32>, DbError> {
    block_on(row_offset_of_async(pool, db_name, table_name, col, value))
}

pub fn object_sql(pool: &Pool, db_name: &str, object_name: &str) -> Result<String, DbError> {
    block_on(object_sql_async(pool, db_name, object_name))
}

pub fn query_free(
    pool: &Pool,
    db_name: &str,
    sql: &str,
    limit: u32,
) -> Result<Vec<String>, DbError> {
    block_on(query_free_async(pool, db_name, sql, limit))
}

pub fn count_free(pool: &Pool, _db_name: &str, sql: &str) -> Result<u32, DbError> {
    block_on(count_free_async(pool, sql))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_to_string_renderiza_todos_los_tipos() {
        assert_eq!(value_to_string(&Value::NULL), "NULL");
        assert_eq!(value_to_string(&Value::Int(-42)), "-42");
        assert_eq!(value_to_string(&Value::UInt(42)), "42");
        assert_eq!(value_to_string(&Value::Float(3.5)), "3.5");
        assert_eq!(value_to_string(&Value::Double(2.25)), "2.25");
        // El protocolo binario envía strings como bytes
        assert_eq!(value_to_string(&Value::Bytes(b"hola".to_vec())), "hola");
        // Bytes que no son UTF-8 no deben paniquear (from_utf8_lossy)
        assert_eq!(value_to_string(&Value::Bytes(vec![0xff, 0xfe])), "\u{fffd}\u{fffd}");
        // Fechas
        assert_eq!(value_to_string(&Value::Date(2026, 8, 2, 13, 5, 9, 0)), "2026-08-02 13:05:09");
        assert_eq!(value_to_string(&Value::Time(true, 1, 2, 3, 4, 0)), "-1d 02:03:04");
    }

    #[test]
    fn rows_convierte_todos_los_valores_y_null() {
        // value_to_string es puro; verificamos la composición con Row
        let v = vec![
            value_to_string(&Value::NULL),
            value_to_string(&Value::Int(7)),
            value_to_string(&Value::Bytes(b"a|b".to_vec())),
        ];
        // Una celda que contiene `|` no se rompe (lección de model.rs)
        assert_eq!(v, vec!["NULL".to_string(), "7".to_string(), "a|b".to_string()]);
    }

    /// Regresión de "Cannot start a runtime from within a runtime": el event
    /// loop de la app corre sobre `#[tokio::main]` (runtime multi-thread), así
    /// que las funciones síncronas del backend se llaman DENTRO de ese runtime.
    /// `block_on` debe reutilizarlo (`block_in_place`) sin crear otro runtime
    /// anidado. Recreamos ese contexto con un runtime multi-thread explícito.
    #[test]
    fn block_on_es_reutilizable_dentro_de_un_runtime() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime multi-thread");
        rt.block_on(async {
            // Dentro del runtime (como en el event loop): no debe paniquear
            // por "runtime within runtime" ni por "block_in_place en single-thread".
            let mut n = 0;
            for i in 0..50 {
                let ok = block_on(async move {
                    let _ = std::hint::black_box(i + 1);
                    true
                });
                n += usize::from(ok);
            }
            assert_eq!(n, 50);
        });
        drop(rt);
        // Fuera de runtime (caso: smoke por env var, #[test] normal): tampoco.
        let x = crate::db::rt::block_on(async { 7 });
        assert_eq!(x, 7);
    }
}
