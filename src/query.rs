use rusqlite::Connection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryState {
    Idle,
    Running,
    Done(Vec<String>),
    Error(String),
}

/// Mensaje del query runner por el canal: (generación, SQL, resultado).
pub type CountMsg = (u64, String, Result<u32, String>);

#[allow(dead_code)]
pub struct QueryResult {
    pub rows: Vec<String>,
    pub error: Option<String>,
}

/// Ejecuta una query SQL de forma asincrónica contra la base de datos
/// Las queries son read-only y se ejecutan en un thread de Tokio para no bloquear la UI
#[allow(dead_code)]
pub async fn execute_query(db_path: &str, sql: &str, limit: u32) -> Result<QueryResult, String> {
    let db_path = db_path.to_string();
    let sql = sql.to_string();

    // Spawn blocking task para no bloquear el event loop
    tokio::task::spawn_blocking(move || {
        let conn =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("Error abriendo DB: {e}"))?;

        let mut stmt = conn.prepare(&sql).map_err(|e| format!("Error parsing SQL: {e}"))?;

        let mut rows = Vec::new();
        let mut count = 0u32;

        // Obtener número de columnas
        let col_count = stmt.column_count();

        // Ejecutar query con LIMIT para evitar cargar todo
        let result = stmt
            .query_map([], |row| {
                let mut row_str = String::new();
                for i in 0..col_count {
                    if i > 0 {
                        row_str.push_str(" | ");
                    }
                    let val: String = row.get(i).unwrap_or_else(|_| "[NULL]".to_string());
                    row_str.push_str(&val);
                }
                Ok(row_str)
            })
            .map_err(|e| format!("Error ejecutando query: {e}"))?;

        for row in result {
            if count >= limit {
                break;
            }
            match row {
                Ok(row_str) => {
                    rows.push(row_str);
                    count += 1;
                }
                Err(e) => {
                    return Err(format!("Error leyendo fila: {e}"));
                }
            }
        }

        Ok(QueryResult { rows, error: None })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Contador de filas con `COUNT(*)` REAL: `SQLite` lo optimiza internamente
/// (no materializa filas, a diferencia de iterar `query_map`). Se ejecuta en
/// un thread de Tokio para no bloquear la UI.
pub async fn count_query_results(db_path: &str, sql: &str) -> Result<u32, String> {
    let db_path = db_path.to_string();
    let sql = sql.to_string();

    tokio::task::spawn_blocking(move || {
        let conn =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("Error abriendo DB: {e}"))?;

        conn.query_row(&sql, [], |row| row.get(0)).map_err(|e| format!("Error contando filas: {e}"))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crea una DB `SQLite` temporal con una tabla de `n` filas y devuelve
    /// (path, cleanup). Los tests del dominio nunca necesitan terminal.
    fn temp_db(name: &str, n: u32) -> (std::path::PathBuf, impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!("lazydb_test_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        let path = dir.join("count.db");
        let conn = Connection::open(&path).expect("abrir db temp");
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
    fn count_query_results_cuenta_filas_reales() {
        let (path, cleanup) = temp_db("count", 5);
        let result = tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(count_query_results(path.to_str().unwrap(), "SELECT COUNT(*) FROM t;"));
        assert_eq!(result, Ok(5));
        cleanup();
    }

    #[test]
    fn count_query_results_errores_no_panican() {
        let (path, cleanup) = temp_db("count_err", 1);
        let result = tokio::runtime::Runtime::new().expect("runtime").block_on(
            count_query_results(path.to_str().unwrap(), "SELECT COUNT(*) FROM no_existe;"),
        );
        assert!(result.is_err(), "tabla inexistente debe dar error, no panic");
        cleanup();
    }
}
