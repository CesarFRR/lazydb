//! Backend `PostgreSQL` (protocolo wire v3 nativo vía `tokio_postgres`).
//!
//! Todas las funciones reciben `&Pool` + `db_name` porque la pool es de
//! larga duración (TCP) y se comparte entre todas las queries de una misma
//! fuente. El adapter (`postgres_adapter.rs`) mantiene la pool lazy-init.
//!
//! Nota de arquitectura: el trait `DbAdapter` es síncrono; `tokio_postgres`
//! es async. Cada función pública bloquea con `crate::db::rt::block_on`
//! (el mismo helper que usa `mysql.rs`).

use std::str::FromStr;

use crate::db::rt::block_on;
use crate::db::{Column, ColumnInfo, DbError, ForeignKey, Row, TableData};
use deadpool_postgres::{Manager, Pool};
use tokio_postgres::NoTls;

// ── Conexión ──────────────────────────────────────────────────────────

/// Crea una pool conectada a `url` (formato `postgres://user:pass@host:port/db`)
/// y devuelve la pool + nombre de la base de datos parseado.
///
/// La BD es OPCIONAL: si falta, la conexión apunta al servidor (útil para
/// listar bases o conectar con la BD por defecto del user).
pub fn connect(url: &str) -> Result<(Pool, String), DbError> {
    let pg_cfg = tokio_postgres::Config::from_str(url)
        .map_err(|e| DbError::Open(format!("URL inválida: {e}")))?;
    let db_name = pg_cfg.get_dbname().unwrap_or_default().to_string();
    let manager = Manager::new(pg_cfg, NoTls);
    let pool = Pool::builder(manager)
        .max_size(5)
        .build()
        .map_err(|e| DbError::Open(format!("pool postgres: {e}")))?;
    Ok((pool, db_name))
}

/// Lista las bases de datos del servidor (`pg_database`), excluyendo las
/// plantillas (`template0`, `template1`). La BD `postgres` (default de
/// mantenimiento) se incluye: es donde aterrizan las instalaciones frescas.
pub fn list_databases(pool: &Pool) -> Result<Vec<String>, DbError> {
    block_on(async {
        let client = pool.get().await?;
        let rows = client
            .query(
                "SELECT datname FROM pg_database \
                 WHERE datistemplate = false ORDER BY datname",
                &[],
            )
            .await?;
        drop(client);
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    })
}

/// El esquema actual (`current_schema()`): el primero del `search_path`,
/// normalmente `public`. Todo el catálogo trabaja con él (la UI es plana,
/// sin árbol de schemas).
async fn current_schema(client: &tokio_postgres::Client) -> Result<String, DbError> {
    let row = client.query_one("SELECT current_schema()", &[]).await?;
    Ok(row.get::<_, String>(0))
}

// ─── Catálogo (tablas, vistas, índices, triggers) ─────────────────────

async fn list_tables_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 AND table_type = 'BASE TABLE' ORDER BY table_name",
            &[&schema],
        )
        .await?;
    drop(client);
    let _ = db_name; // el nombre de BD ya está embebido en la conexión
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

async fn list_views_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 AND table_type = 'VIEW' ORDER BY table_name",
            &[&schema],
        )
        .await?;
    drop(client);
    let _ = db_name;
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

async fn list_indexes_async(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = $1 AND indexname NOT LIKE '%_pkey' \
             ORDER BY indexname",
            &[&schema],
        )
        .await?;
    drop(client);
    let _ = db_name;
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

// Nota: `PostgreSQL` no tiene triggers listables de forma trivial para la UI
// plana (`pg_trigger` existe pero requiere cast de `oid`). El adapter devuelve
// vacío para "trigger".
// ─── Metadata de columnas ─────────────────────────────────────────────

async fn column_info_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ColumnInfo>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT ordinal_position, column_name, data_type, is_nullable,
                    COALESCE((SELECT 'PRI' FROM information_schema.table_constraints tc
                              JOIN information_schema.key_column_usage kcu
                                ON tc.constraint_name = kcu.constraint_name
                               WHERE tc.constraint_type = 'PRIMARY KEY'
                                 AND tc.table_schema = $1 AND tc.table_name = $2
                                 AND kcu.column_name = c.column_name
                                 AND kcu.table_schema = $1 AND kcu.table_name = $2
                               LIMIT 1), '')
             FROM information_schema.columns c
             WHERE table_schema = $1 AND table_name = $2
             ORDER BY ordinal_position",
            &[&schema, &table_name],
        )
        .await?;
    drop(client);
    let _ = db_name;
    Ok(rows
        .into_iter()
        .map(|r| ColumnInfo {
            cid: i64::from(r.get::<_, i32>(0)) - 1,
            name: r.get::<_, String>(1),
            dtype: r.get::<_, String>(2),
            notnull: r.get::<_, String>(3).eq_ignore_ascii_case("NO"),
            pk: r.get::<_, String>(4).eq_ignore_ascii_case("PRI"),
        })
        .collect())
}

// ─── Filas (inspector, Data tab, query libre) ─────────────────────────
//
// Usamos `simple_query` en vez de `client.query` porque el formato binario
// que usa el protocolo v3 no permite decodificar `uuid`, `jsonb`, `point`,
// `tstzrange`, arrays ni enums de vuelta a `String` con
// `try_get::<Option<&str>>`.  `simple_query` siempre pide al servidor que
// envíe TEXTO crudo → UUID como `dc7c6392-...`, jsonb como `{"os":…}`,
// arrays como `{rust}`, etc. El servidor convierte todos los tipos de
// usuario sin que necesitemos cascadas manuales.

fn rows_from_simple_rows(msgs: Vec<tokio_postgres::SimpleQueryMessage>) -> Vec<Row> {
    msgs.into_iter()
        .filter_map(|msg| match msg {
            tokio_postgres::SimpleQueryMessage::Row(row) => Some(Row {
                cells: (0..row.len()).map(|i| row.get(i).unwrap_or("NULL").to_string()).collect(),
            }),
            _ => None,
        })
        .collect()
}

async fn rows_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let sql = format!("SELECT * FROM \"{schema}\".\"{table_name}\" LIMIT {limit} OFFSET {offset}");
    let msgs = client.simple_query(&sql).await?;
    drop(client);
    let _ = db_name;
    Ok(rows_from_simple_rows(msgs))
}

async fn table_rows_sorted_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
    order_col: Option<(&str, bool)>,
) -> Result<TableData, DbError> {
    let columns = column_names_async(pool, db_name, table_name).await?;

    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let order_clause = if let Some((col, asc)) = order_col {
        let dir = if asc { "ASC" } else { "DESC" };
        format!(" ORDER BY \"{col}\" {dir}")
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT * FROM \"{schema}\".\"{table_name}\"{order_clause} LIMIT {limit} OFFSET {offset}"
    );
    let rows = client.simple_query(&sql).await?;
    drop(client);
    Ok(TableData { columns, rows: rows_from_simple_rows(rows) })
}

async fn row_count_async(pool: &Pool, db_name: &str, table_name: &str) -> Result<u32, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let sql = format!("SELECT COUNT(*) FROM \"{schema}\".\"{table_name}\"");
    let row = client.query_one(&sql, &[]).await?;
    let count: i64 = row.get(0);
    drop(client);
    let _ = db_name;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count.max(0) as u32)
}

async fn column_names_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<Column>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT column_name, data_type FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
            &[&schema, &table_name],
        )
        .await?;
    drop(client);
    let _ = db_name;
    Ok(rows.into_iter().map(|r| Column { name: r.get(0), dtype: r.get(1) }).collect())
}

async fn fk_async(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
) -> Result<Vec<ForeignKey>, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let rows = client
        .query(
            "SELECT kcu.column_name, ccu.table_name, ccu.column_name,
                    kcu.ordinal_position, kcu.constraint_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
             JOIN information_schema.constraint_column_usage ccu
               ON ccu.constraint_name = tc.constraint_name
              AND ccu.table_schema = tc.table_schema
             WHERE tc.constraint_type = 'FOREIGN KEY'
               AND tc.table_schema = $1 AND tc.table_name = $2
             ORDER BY kcu.ordinal_position",
            &[&schema, &table_name],
        )
        .await?;
    drop(client);
    let _ = db_name;
    Ok(rows
        .into_iter()
        .map(|r| ForeignKey {
            id: 0,
            seq: i64::from(r.get::<_, i32>(3)),
            table: r.get::<_, String>(1),
            from: r.get::<_, String>(0),
            to: Some(r.get::<_, String>(2)),
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
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    let sql =
        format!("SELECT COUNT(*) FROM \"{schema}\".\"{table_name}\" WHERE \"{col}\" < '{value}'");
    let row = client.query_one(&sql, &[]).await?;
    let count: i64 = row.get(0);
    drop(client);
    let _ = db_name;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(if count > 0 { Some(count as u32 - 1) } else { None })
}

async fn object_sql_async(
    pool: &Pool,
    db_name: &str,
    object_name: &str,
) -> Result<String, DbError> {
    let client = pool.get().await?;
    let schema = current_schema(&client).await?;
    // Vistas → definición real; tablas → CREATE TABLE reconstruido.
    let is_view = {
        let r = client
            .query_opt(
                "SELECT 1 FROM information_schema.tables \
                 WHERE table_schema = $1 AND table_name = $2 AND table_type = 'VIEW'",
                &[&schema, &object_name],
            )
            .await?;
        r.is_some()
    };
    let ddl = if is_view {
        let r = client
            .query_one(
                "SELECT pg_get_viewdef($1::regclass, true)",
                &[&format!("{schema}.{object_name}")],
            )
            .await?;
        format!("CREATE VIEW \"{object_name}\" AS\n{}", r.get::<_, String>(0))
    } else {
        let rows = client
            .query(
                "SELECT column_name, data_type, is_nullable,
                        COALESCE((SELECT 'PRI' FROM information_schema.table_constraints tc
                                  JOIN information_schema.key_column_usage kcu
                                    ON tc.constraint_name = kcu.constraint_name
                                   WHERE tc.constraint_type = 'PRIMARY KEY'
                                     AND tc.table_schema = $1 AND tc.table_name = $2
                                     AND kcu.column_name = c.column_name
                                     AND kcu.table_schema = $1 AND kcu.table_name = $2
                                   LIMIT 1), '')
                 FROM information_schema.columns c
                 WHERE table_schema = $1 AND table_name = $2
                 ORDER BY ordinal_position",
                &[&schema, &object_name],
            )
            .await?;
        let cols: Vec<(String, String, String, String)> =
            rows.into_iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect();
        let mut lines: Vec<String> = cols
            .iter()
            .map(|(name, dtype, nullable, _pk)| {
                let nn = if nullable.eq_ignore_ascii_case("NO") { " NOT NULL" } else { "" };
                format!("    \"{name}\" {dtype}{nn}")
            })
            .collect();
        let pks: Vec<&str> = cols
            .iter()
            .filter(|(_n, _t, _nu, pk)| pk == "PRI")
            .map(|(n, _t, _nu, _pk)| n.as_str())
            .collect();
        if !pks.is_empty() {
            lines.push(format!("    PRIMARY KEY ({})", pks.join(", ")));
        }
        format!("CREATE TABLE \"{object_name}\" (\n{}\n)", lines.join(",\n"))
    };
    drop(client);
    let _ = db_name;
    Ok(ddl)
}

async fn query_free_async(
    pool: &Pool,
    _db_name: &str,
    sql: &str,
    limit: u32,
) -> Result<Vec<String>, DbError> {
    let client = pool.get().await?;
    // Solo añadimos LIMIT a SELECTs simples sin LIMIT propio.
    let trimmed = sql.trim();
    let lower = trimmed.to_ascii_lowercase();
    let is_select = lower.starts_with("select") || lower.starts_with("with");
    let has_limit = lower.contains(" limit ");
    let row_sql = if is_select && !has_limit {
        format!("{trimmed} LIMIT {limit}")
    } else {
        trimmed.to_string()
    };
    let rows = rows_from_simple_rows(client.simple_query(&row_sql).await?);
    drop(client);
    Ok(rows.into_iter().map(|r| r.cells.join(" | ")).collect())
}

async fn count_free_async(pool: &Pool, sql: &str) -> Result<u32, DbError> {
    let client = pool.get().await?;
    let row = client.query_one(sql, &[]).await?;
    let count: i64 = row.get(0);
    drop(client);
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(count.max(0) as u32)
}

// ─── Wrappers síncronos (el trait NO es async) ────────────────────────

pub fn list_tables(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_tables_async(pool, db_name))
}

pub fn list_views(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_views_async(pool, db_name))
}

pub fn list_all_indexes(pool: &Pool, db_name: &str) -> Result<Vec<String>, DbError> {
    block_on(list_indexes_async(pool, db_name))
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

/// Filas con celdas expandidas (multilínea) para el inspector de fila:
/// `JSONB` y `JSON` → pretty de `serde_json`; arrays (`{a,b}`) → estilo numpy.
pub fn table_data_rows_pretty(
    pool: &Pool,
    db_name: &str,
    table_name: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<Row>, DbError> {
    let rows = table_data_rows(pool, db_name, table_name, limit, offset)?;
    Ok(crate::db::pretty::prettify_rows(rows))
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

    /// Regresión del helper `block_on` compartido: reutiliza el runtime activo
    /// (multi-thread, como el de la app) sin abrir uno anidado.
    #[test]
    fn block_on_reutiliza_runtime_en_postgres_path() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime multi-thread");
        rt.block_on(async {
            let n = crate::db::rt::block_on(async { 42 });
            assert_eq!(n, 42);
        });
        drop(rt);
        let x = crate::db::rt::block_on(async { 7 });
        assert_eq!(x, 7);
    }

    /// Smoke contra `PostgreSQL` real. Requiere el servidor local y la env var
    /// `LAZYDB_POSTGRES_URL` con una BD que tenga la tabla `categories`
    /// (p. ej. `postgres://postgres@127.0.0.1:5432/lazydb_demo`).
    /// Ver `AGENTS.md` para levantar el servicio (systemd).
    #[test]
    #[ignore = "requiere PostgreSQL local + LAZYDB_POSTGRES_URL"]
    fn smoke_postgres_localhost() {
        let Some(url) = std::env::var("LAZYDB_POSTGRES_URL").ok() else {
            eprintln!("    ⚠︎ LAZYDB_POSTGRES_URL no definida — omitiendo smoke PostgreSQL");
            return;
        };
        println!("=== {url} (host y user ocultos por privacidad) ===");
        let adapter = crate::db::resolver::resolve_backend(&url)
            .expect("resolver debe reconocer postgres://");

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
            Ok(adv) => println!("  AVANZADOS (índices): {adv:?}"),
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
        match adapter.query("SELECT * FROM categories ORDER BY id LIMIT 3", 5) {
            Ok(rows) => println!("  QUERY LIBRE: {rows:?}"),
            Err(err) => println!("  ERROR QUERY: {err:?}"),
        }

        // Conexión a nivel de SERVIDOR (sin BD): listar bases disponibles.
        // Quitamos la BD de la URL (`.../lazydb_demo` → `.../`) para conectar
        // solo al host y hacer SELECT de pg_database.
        let trimmed = url.trim_end_matches('/');
        let server_url = trimmed
            .rfind('/')
            .map_or_else(|| trimmed.to_string(), |idx| trimmed[..=idx].to_string());
        match crate::db::backends::postgres::connect(&server_url) {
            Ok((pool, db_name)) => {
                println!("  SERVER connect db_name='{db_name}'");
                match crate::db::backends::postgres::list_databases(&pool) {
                    Ok(dbs) => println!("  BASES: {dbs:?}"),
                    Err(err) => println!("  ERROR BASES: {err:?}"),
                }
            }
            Err(err) => println!("  ERROR SERVER connect: {err:?}"),
        }
    }

    /// Smoke del inspector pretty contra `user_profiles` (BD de prueba con
    /// tipos avanzados: uuid, jsonb, text[], point, tstzrange, enum).
    /// Requiere la misma env var que `smoke_postgres_localhost`.
    #[test]
    #[ignore = "requiere PostgreSQL local + LAZYDB_POSTGRES_URL"]
    fn smoke_row_inspector_pretty_postgres() {
        let Some(url) = std::env::var("LAZYDB_POSTGRES_URL").ok() else {
            eprintln!("    ⚠︎ LAZYDB_POSTGRES_URL no definida — omitiendo smoke pretty");
            return;
        };
        println!("=== PRETTY {url} ===");
        let adapter = crate::db::resolver::resolve_backend(&url)
            .expect("resolver debe reconocer postgres://");

        // Compacto vs pretty: el pretty debe expandir JSON y arrays
        let compact = adapter.table_data_rows("user_profiles", 3, 0);
        let pretty = adapter.table_data_rows_pretty("user_profiles", 3, 0);
        match (compact, pretty) {
            (Ok(compact_rows), Ok(pretty_rows)) => {
                println!("  COMPACTO: {compact_rows:?}");
                println!("  PRETTY:   {pretty_rows:?}");
                assert!(!pretty_rows.is_empty(), "user_profiles debe tener filas");
                // El pretty debe ser MULTILÍNEA (JSON/arrays expandidos) en
                // alguna celda → contiene '\n' que el compacto no tiene.
                let compact_flat: String =
                    compact_rows.iter().flat_map(|r| r.cells.iter()).cloned().collect();
                let pretty_flat: String =
                    pretty_rows.iter().flat_map(|r| r.cells.iter()).cloned().collect();
                assert_ne!(compact_flat, pretty_flat, "pretty debe diferir del compacto");
                assert!(
                    pretty_flat.contains('\n'),
                    "pretty debe tener celdas multilínea (JSON/array): {pretty_flat}"
                );
                println!("  ✔ pretty expande tipos complejos");
            }
            (Err(err), _) => println!("  ERROR COMPACTO: {err:?}"),
            (_, Err(err)) => println!("  ERROR PRETTY: {err:?}"),
        }
    }
}
