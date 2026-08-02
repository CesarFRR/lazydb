//! Error tipado del dominio de bases de datos.
//!
//! Los mensajes se guardan como `String` plano (no el error original):
//! - el status bar y `preview_rows` los muestran directo (`Display`),
//! - los tests pueden comparar con `assert_eq` (`PartialEq`),
//! - los `From` impls hacen que `?` convierta automáticamente los errores
//!   de rusqlite, E/S y tareas de Tokio.

/// Error del dominio de bases de datos (fuera el `Result<_, String>`).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DbError {
    /// No se pudo abrir la base de datos (incluye el path en el mensaje).
    #[error("Error abriendo la base de datos: {0}")]
    Open(String),
    /// Error de `SQLite` (parseo, ejecución, lectura de fila, ...).
    #[error("Error de SQLite: {0}")]
    Sqlite(String),
    /// Error de E/S del sistema.
    #[error("Error de E/S: {0}")]
    Io(String),
    /// Error en la tarea en segundo plano (join de Tokio).
    #[error("Error en la tarea en segundo plano: {0}")]
    Join(String),
}

impl From<rusqlite::Error> for DbError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err.to_string())
    }
}

impl From<duckdb::Error> for DbError {
    fn from(err: duckdb::Error) -> Self {
        Self::Sqlite(err.to_string())
    }
}

impl From<std::io::Error> for DbError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<tokio::task::JoinError> for DbError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Join(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_incluye_contexto_de_la_variante() {
        let err = DbError::Open("/tmp/x.db: unable to open database file".to_string());
        assert_eq!(
            err.to_string(),
            "Error abriendo la base de datos: /tmp/x.db: unable to open database file"
        );
    }

    #[test]
    fn from_convierte_errores_de_rusqlite() {
        let msg = DbError::from(rusqlite::Error::InvalidQuery).to_string();
        assert!(
            msg.starts_with("Error de SQLite") && msg.len() > "Error de SQLite: ".len(),
            "el mensaje de rusqlite debe preservarse: {msg}"
        );
    }
}
