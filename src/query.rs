use crate::db::DbError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryState {
    Idle,
    Running,
    Done(Vec<String>),
    Error(String),
}

/// Mensaje del query runner por el canal: COUNT(*) o query libre del usuario.
pub enum QueryMsg {
    /// (generación, SQL, resultado del COUNT)
    Count(u64, String, Result<u32, DbError>),
    /// (generación, SQL, resultado de query libre)
    Free(u64, String, Result<QueryResult, DbError>),
}

/// Tope de filas para una query libre (filosofía culling: nunca materializar
/// la DB entera en el preview).
pub const QUERY_RESULT_LIMIT: u32 = 500;

#[allow(dead_code)]
pub struct QueryResult {
    pub rows: Vec<String>,
    pub error: Option<String>,
}

/// Ejecuta una query SQL de forma asincrónica contra la base de datos.
/// Las queries son read-only y se ejecutan en un thread de Tokio para no
/// bloquear la UI.
///
/// `adapter`: conexión activa del provider (se reusa — una sola conexión,
/// evita re-handshakes en DBs online). `None` → resuelve por `db_path`
/// (fallback para tests y recientes).
pub async fn execute_query(
    adapter: Option<std::sync::Arc<dyn crate::db::adapter::DbAdapter>>,
    db_path: &str,
    sql: &str,
    limit: u32,
) -> Result<QueryResult, DbError> {
    let sql = sql.to_string();
    let db_path = db_path.to_string();

    // Spawn blocking task para no bloquear el event loop
    tokio::task::spawn_blocking(move || {
        let rows = if let Some(a) = adapter {
            a.query(&sql, limit)?
        } else {
            let a = crate::db::resolver::resolve_backend(&db_path)
                .ok_or_else(|| DbError::Open(format!("{db_path}: fuente no soportada")))?;
            a.query(&sql, limit)?
        };
        Ok(QueryResult { rows, error: None })
    })
    .await?
}

/// Contador de filas con `COUNT(*)` REAL: el backend lo optimiza internamente
/// (no materializa filas, a diferencia de iterar `query_map`). Se ejecuta en
/// un thread de Tokio para no bloquear la UI.
pub async fn count_query_results(
    adapter: Option<std::sync::Arc<dyn crate::db::adapter::DbAdapter>>,
    db_path: &str,
    sql: &str,
) -> Result<u32, DbError> {
    let sql = sql.to_string();
    let db_path = db_path.to_string();

    tokio::task::spawn_blocking(move || -> Result<u32, DbError> {
        if let Some(a) = adapter {
            a.count(&sql)
        } else {
            let a = crate::db::resolver::resolve_backend(&db_path)
                .ok_or_else(|| DbError::Open(format!("{db_path}: fuente no soportada")))?;
            a.count(&sql)
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "sqlite", feature = "duckdb"))]
    use super::*;

    /// Crea una DB `SQLite` temporal con una tabla de `n` filas y devuelve
    /// (path, cleanup). Los tests del dominio nunca necesitan terminal.
    #[cfg(feature = "sqlite")]
    fn temp_db(name: &str, n: u32) -> (std::path::PathBuf, impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!("lazydb_test_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("count.db");
        let conn = rusqlite::Connection::open(&path).expect("abrir db temp");
        conn.execute_batch("CREATE TABLE t (a INTEGER);").expect("crear tabla");
        for i in 0..n {
            conn.execute("INSERT INTO t (a) VALUES (?1)", [i]).expect("insertar fila");
        }
        drop(conn);
        let cleanup_path = path.clone();
        let cleanup = move || {
            let _ = std::fs::remove_file(&cleanup_path);
            let _ = std::fs::remove_dir(&dir);
        };
        (path, cleanup)
    }

    /// Crea una DB `DuckDB` temporal con la misma tabla (para probar que el
    /// query runner despacha por extensión).
    #[cfg(feature = "duckdb")]
    fn temp_db_duck(name: &str, n: u32) -> (std::path::PathBuf, impl FnOnce()) {
        let dir =
            std::env::temp_dir().join(format!("lazydb_test_ddb_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("count.duckdb");
        let conn = duckdb::Connection::open(&path).expect("abrir db temp");
        conn.execute_batch("CREATE TABLE t (a INTEGER);").expect("crear tabla");
        for i in 0..n {
            conn.execute("INSERT INTO t (a) VALUES (?1)", [i]).expect("insertar fila");
        }
        drop(conn);
        let cleanup_path = path.clone();
        let cleanup = move || {
            let _ = std::fs::remove_file(&cleanup_path);
            let _ = std::fs::remove_dir(&dir);
        };
        (path, cleanup)
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn count_query_results_cuenta_filas_reales() {
        let (path, cleanup) = temp_db("count", 5);
        let result = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(count_query_results(None, path.to_str().unwrap(), "SELECT COUNT(*) FROM t;"));
        assert_eq!(result, Ok(5));
        cleanup();
    }

    #[test]
    #[cfg(feature = "duckdb")]
    fn count_query_results_cuenta_en_duckdb() {
        let (path, cleanup) = temp_db_duck("count_ddb", 7);
        let result = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(count_query_results(None, path.to_str().unwrap(), "SELECT COUNT(*) FROM t;"));
        assert_eq!(result, Ok(7));
        cleanup();
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn count_query_results_errores_no_panican() {
        let (path, cleanup) = temp_db("count_err", 1);
        let result = tokio::runtime::Runtime::new().expect("runtime").block_on(
            count_query_results(None, path.to_str().unwrap(), "SELECT COUNT(*) FROM no_existe;"),
        );
        assert!(result.is_err(), "tabla inexistente debe dar error, no panic");
        cleanup();
    }

    #[test]
    #[cfg(feature = "duckdb")]
    fn execute_query_despacha_por_extension_a_duckdb() {
        let (path, cleanup) = temp_db_duck("query_ddb", 3);
        let result = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(execute_query(None, path.to_str().unwrap(), "SELECT a FROM t ORDER BY a", 10))
            .expect("query ok");
        assert_eq!(result.rows, vec!["0", "1", "2"]);
        cleanup();
    }

    #[test]
    #[cfg(feature = "sqlite")]
    fn execute_query_devuelve_error_sin_panico() {
        let (path, cleanup) = temp_db("query_err", 1);
        let result = tokio::runtime::Runtime::new().expect("runtime").block_on(execute_query(
            None,
            path.to_str().unwrap(),
            "SELECT * FROM no_existe",
            10,
        ));
        assert!(result.is_err(), "tabla inexistente debe dar error, no panic");
        cleanup();
    }
}
