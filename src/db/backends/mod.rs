// Backends concretos, gateados por feature (Fase B):
// `sqlite` → sqlite.rs/adapter, `duckdb` → duckdb.rs/adapter,
// `files` → file.rs/adapter (depende de duckdb), `mysql` → mysql.rs/adapter,
// `postgres` → postgres.rs/adapter.
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub mod sqlite_adapter;

#[cfg(feature = "duckdb")]
pub mod duckdb;
#[cfg(feature = "duckdb")]
pub mod duckdb_adapter;

#[cfg(feature = "files")]
pub mod file;
#[cfg(feature = "files")]
pub mod file_adapter;

#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "mysql")]
pub mod mysql_adapter;

#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "postgres")]
pub mod postgres_adapter;
