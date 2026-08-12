// Resolver/factory que el controller puede usar para obtener un backend
// concreto a partir de una cadena de conexión o path.
use crate::db::adapter::DbAdapter;

#[cfg(feature = "duckdb")]
use crate::db::backends::duckdb_adapter::DuckdbAdapter;
#[cfg(feature = "files")]
use crate::db::backends::file_adapter::FileAdapter;
#[cfg(feature = "mongodb")]
use crate::db::backends::mongo_adapter::MongoAdapter;
#[cfg(feature = "mysql")]
use crate::db::backends::mysql_adapter::MysqlAdapter;
#[cfg(feature = "postgres")]
use crate::db::backends::postgres_adapter::PostgresAdapter;
#[cfg(feature = "sqlite")]
use crate::db::backends::sqlite_adapter::SqliteAdapter;

/// Devuelve un adaptador para la fuente indicada o None si no se puede resolver.
/// Detecta paths locales (*.db → sqlite, *.duckdb/*.ddb → duckdb,
/// *.csv/*.parquet/*.json/*.geojson/*.gpkg → archivo de datos) o URLs
/// con prefijo `sqlite://` / `duckdb://` / `mysql://` / `postgres://` /
/// `mongodb://`.
pub fn resolve_backend(source: &str) -> Option<Box<dyn DbAdapter>> {
    #[cfg(feature = "sqlite")]
    if let Some(rest) = source.strip_prefix("sqlite://") {
        return Some(Box::new(SqliteAdapter::new(rest)));
    }

    #[cfg(feature = "duckdb")]
    if let Some(rest) = source.strip_prefix("duckdb://") {
        return Some(Box::new(DuckdbAdapter::new(rest)));
    }

    #[cfg(feature = "mysql")]
    if source.starts_with("mysql://") {
        return MysqlAdapter::new(source).map(|a| Box::new(a) as Box<dyn DbAdapter>).ok();
    }

    #[cfg(feature = "postgres")]
    if source.starts_with("postgres://") {
        return PostgresAdapter::new(source).map(|a| Box::new(a) as Box<dyn DbAdapter>).ok();
    }

    #[cfg(feature = "mongodb")]
    if source.starts_with("mongodb://") {
        return MongoAdapter::new(source).map(|a| Box::new(a) as Box<dyn DbAdapter>).ok();
    }

    if let Some(ext) = std::path::Path::new(source).extension() {
        #[cfg(feature = "duckdb")]
        if ext.eq_ignore_ascii_case("duckdb") || ext.eq_ignore_ascii_case("ddb") {
            return Some(Box::new(DuckdbAdapter::new(source)));
        }
        #[cfg(feature = "sqlite")]
        if ext.eq_ignore_ascii_case("db") {
            return Some(Box::new(SqliteAdapter::new(source)));
        }
        // Archivos de datos locales (un dataset virtual cada uno)
        #[cfg(feature = "files")]
        if crate::db::backends::file::kind_for(source).is_some() {
            return Some(Box::new(FileAdapter::new(source)));
        }
        #[cfg(not(any(feature = "duckdb", feature = "sqlite", feature = "files")))]
        let _ = ext;
    }

    #[cfg(feature = "sqlite")]
    if source.starts_with('/') {
        return Some(Box::new(SqliteAdapter::new(source)));
    }

    None
}
