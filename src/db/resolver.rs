// Resolver/factory que el controller puede usar para obtener un backend
// concreto a partir de una cadena de conexión o path.
use crate::db::adapter::DbAdapter;
use crate::db::backends::sqlite_adapter::SqliteAdapter;

/// Devuelve un adaptador para la fuente indicada o None si no se puede resolver.
/// Detecta un path local *.db o URLs que comiencen por `sqlite://`.
#[allow(dead_code)]
pub fn resolve_backend(source: &str) -> Option<Box<dyn DbAdapter>> {
    if let Some(rest) = source.strip_prefix("sqlite://") {
        return Some(Box::new(SqliteAdapter::new(rest)));
    }

    if std::path::Path::new(source).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("db"))
        || source.starts_with('/')
    {
        return Some(Box::new(SqliteAdapter::new(source)));
    }

    None
}
