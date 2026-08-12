pub mod adapter;
pub mod backends;
pub mod connection;
pub mod error;
pub mod model;
pub mod pretty;
pub mod query_guard;
pub mod resolver;
// Runtime tokio para los drivers async (`mysql`, `postgres`, `mongodb`).
// Los backends locales (sqlite/duckdb/files) son sync puro y no lo necesitan.
#[cfg(any(feature = "mysql", feature = "postgres", feature = "mongodb"))]
pub mod rt;
pub mod servers;

pub use error::DbError;
pub use model::{Column, ColumnInfo, ForeignKey, Row, TableData};
