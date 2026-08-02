// Resolver/factory que el controller puede usar para obtener un backend
// concreto a partir de una cadena de conexión o path.
use crate::db::adapter::DbAdapter;
use crate::db::backends::duckdb_adapter::DuckdbAdapter;
use crate::db::backends::sqlite_adapter::SqliteAdapter;

/// Devuelve un adaptador para la fuente indicada o None si no se puede resolver.
/// Detecta paths locales (*.db → sqlite, *.duckdb/*.ddb → duckdb) o URLs
/// con prefijo `sqlite://` / `duckdb://`.
#[allow(dead_code)]
pub fn resolve_backend(source: &str) -> Option<Box<dyn DbAdapter>> {
    if let Some(rest) = source.strip_prefix("sqlite://") {
        return Some(Box::new(SqliteAdapter::new(rest)));
    }

    if let Some(rest) = source.strip_prefix("duckdb://") {
        return Some(Box::new(DuckdbAdapter::new(rest)));
    }

    if let Some(ext) = std::path::Path::new(source).extension() {
        if ext.eq_ignore_ascii_case("duckdb") || ext.eq_ignore_ascii_case("ddb") {
            return Some(Box::new(DuckdbAdapter::new(source)));
        }
        if ext.eq_ignore_ascii_case("db") {
            return Some(Box::new(SqliteAdapter::new(source)));
        }
    }

    if source.starts_with('/') {
        return Some(Box::new(SqliteAdapter::new(source)));
    }

    None
}
