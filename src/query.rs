use rusqlite::Connection;

#[derive(Clone, Debug)]
pub enum QueryState {
    Idle,
    Running,
    #[allow(dead_code)]
    Done(Vec<String>),
    #[allow(dead_code)]
    Error(String),
}

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

/// Contador de filas (para indicador "X filas encontradas")
#[allow(dead_code)]
pub async fn count_query_results(db_path: &str, sql: &str) -> Result<u32, String> {
    let db_path = db_path.to_string();
    let sql = sql.to_string();

    tokio::task::spawn_blocking(move || {
        let conn =
            Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("Error abriendo DB: {e}"))?;

        let mut count = 0u32;
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("Error parsing SQL: {e}"))?;

        let result =
            stmt.query_map([], |_| Ok(())).map_err(|e| format!("Error ejecutando query: {e}"))?;

        for _ in result {
            count += 1;
        }

        Ok(count)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}
