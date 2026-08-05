//! Analizador de conexiones: dado un input (URL completa o path de archivo)
//! detecta QUÉ tipo de base es, sin crear el adapter. Alimenta el formulario
//! de "Nueva conexión" del panel Detail (auto-detección en vivo) y es la
//! fuente de verdad de `resolve_backend` (que además crea el adapter).
//!
//! Reglas (en orden):
//! 1. URL con scheme `mysql://` / `postgres://` / `postgresql://` /
//!    `mongodb://` → tipo remoto, host/port/db parseados.
//! 2. URL `sqlite://` / `duckdb://` → path de archivo con scheme.
//! 3. Path con extensión conocida (.db/.sqlite → sqlite, .duckdb/.ddb →
//!    duckdb, csv/tsv/parquet/json/jsonl/geojson/gpkg → archivo de datos).
//! 4. Path absoluto (`/...`) sin extensión conocida → sqlite por defecto
//!    (compatibilidad con el comportamiento actual del resolver).

use std::path::Path;

/// Tipos de conexión detectables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionType {
    Mysql,
    Postgres,
    Mongo,
    Sqlite,
    Duckdb,
    File,
    Unknown,
}

impl ConnectionType {
    /// Etiqueta corta para la UI (selector y auto-detección).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mysql => "MySQL",
            Self::Postgres => "PostgreSQL",
            Self::Mongo => "MongoDB",
            Self::Sqlite => "SQLite",
            Self::Duckdb => "DuckDB",
            Self::File => "Archivo de datos",
            Self::Unknown => "Auto (detectar)",
        }
    }
}

/// Especificación parseada de una conexión.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionSpec {
    pub kind: ConnectionType,
    /// `true` si el input es una URL (`scheme://`), `false` si es un path.
    pub is_url: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub db_name: Option<String>,
    /// Path de archivo resuelto (URLs `sqlite://`/`duckdb://` o paths).
    pub file_path: Option<String>,
}

impl ConnectionSpec {
    /// Descripción humana para el formulario (p.ej. `MySQL · localhost:3306/lazy`).
    pub fn display(&self) -> String {
        match (&self.db_name, &self.file_path) {
            (Some(db), _) => format!("{} · {}/{}", self.kind.label(), self.host_display(), db),
            (_, Some(path)) => format!("{} · {}", self.kind.label(), path),
            _ => format!("{} · {}", self.kind.label(), self.host_display()),
        }
    }

    fn host_display(&self) -> String {
        match (&self.host, self.port) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.clone(),
            _ => "-".to_string(),
        }
    }
}

/// Analiza un input (URL o path) y devuelve el tipo detectado.
pub fn analyze_connection(input: &str) -> ConnectionSpec {
    let input = input.trim();
    if input.is_empty() {
        return ConnectionSpec {
            kind: ConnectionType::Unknown,
            is_url: false,
            host: None,
        user: None,
        pass: None,
            port: None,
            db_name: None,
            file_path: None,
        };
    }

    // ── 1. URLs remotas con scheme ──
    if let Some((scheme, rest)) = input.split_once("://") {
        let scheme_lower = scheme.to_ascii_lowercase();
        match scheme_lower.as_str() {
            "mysql" => return parse_remote(input, ConnectionType::Mysql, Some(3306)),
            "postgres" | "postgresql" => {
                return parse_remote(input, ConnectionType::Postgres, Some(5432))
            }
            "mongodb" => return parse_remote(input, ConnectionType::Mongo, Some(27017)),
            "sqlite" => {
                return ConnectionSpec {
                    kind: ConnectionType::Sqlite,
                    is_url: true,
                    host: None,
        user: None,
        pass: None,
                    port: None,
                    db_name: None,
                    file_path: Some(rest.to_string()),
                };
            }
            "duckdb" => {
                return ConnectionSpec {
                    kind: ConnectionType::Duckdb,
                    is_url: true,
                    host: None,
        user: None,
        pass: None,
                    port: None,
                    db_name: None,
                    file_path: Some(rest.to_string()),
                };
            }
            _ => { /* scheme desconocido → tratarlo como texto libre */ }
        }
    }

    // ── 2. Paths locales por extensión ──
    if let Some(ext) = Path::new(input).extension().and_then(|e| e.to_str()) {
        let kind = match ext.to_ascii_lowercase().as_str() {
            "db" | "sqlite" | "sqlite3" => ConnectionType::Sqlite,
            "duckdb" | "ddb" => ConnectionType::Duckdb,
            "csv" | "tsv" | "parquet" | "pq" | "json" | "jsonl" | "ndjson"
            | "geojson" | "gpkg" => ConnectionType::File,
            _ => ConnectionType::Unknown,
        };
        if kind != ConnectionType::Unknown {
            return ConnectionSpec {
                kind,
                is_url: false,
                host: None,
        user: None,
        pass: None,
                port: None,
                db_name: None,
                file_path: Some(input.to_string()),
            };
        }
    }

    // ── 3. Path absoluto sin extensión conocida → sqlite por defecto ──
    if input.starts_with('/') || input.starts_with('~') || input.starts_with("./") {
        return ConnectionSpec {
            kind: ConnectionType::Sqlite,
            is_url: false,
            host: None,
        user: None,
        pass: None,
            port: None,
            db_name: None,
            file_path: Some(input.to_string()),
        };
    }

    ConnectionSpec {
        kind: ConnectionType::Unknown,
        is_url: false,
        host: None,
        user: None,
        pass: None,
        port: None,
        db_name: None,
        file_path: None,
    }
}

/// Parsea una URL remota (`mysql://user:pass@host:3306/db`) extrayendo
/// user, pass, host, puerto (default si no viene) y base.
///
/// Casos límite:
/// - Password con `@`: `user:pa@ss@host` → se separa con el ÚLTIMO `@`.
/// - Password con `:`: `user:pa:ss@host` → el primer `:` separa user/pass.
/// - Puerto ausente: default del scheme (`mysql`→3306, `postgres`→5432...).
fn parse_remote(input: &str, kind: ConnectionType, default_port: Option<u16>) -> ConnectionSpec {
    let rest = input.split_once("://").map_or(input, |(_, r)| r);

    // `user:pass@host...` → credenciales y el resto
    let (user_part, host_part) = rest.rsplit_once('@').map_or((None, rest), |(creds, host)| {
        // `user:pass` → primer `:` separa (la pass puede contener `:`)
        let (u, p) = creds.split_once(':').unwrap_or((creds, ""));
        (Some((u, p)), host)
    });
    let (user, pass) = user_part.map_or((None, None), |(u, p)| {
        (Some(u.to_string()), Some(p.to_string()))
    });

    // `host:port/db` o `host/db`
    let (host_and_port, db_name) = host_part.split_once('/').map_or((host_part, None), |(hp, d)| {
        (hp, Some(d.to_string()))
    });
    let (host, port) = host_and_port.rfind(':').map_or((host_and_port, default_port), |colon| {
        let h = &host_and_port[..colon];
        let p = host_and_port[colon + 1..].parse::<u16>().ok().or(default_port);
        (h, p)
    });

    ConnectionSpec {
        kind,
        is_url: true,
        host: Some(host.to_string()),
        port,
        user: user.filter(|u| !u.is_empty()),
        pass: pass.filter(|p| !p.is_empty()),
        db_name: db_name.filter(|d| !d.is_empty()),
        file_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecta_urls_remotas() {
        let spec = analyze_connection("mysql://user:pass@localhost:3306/lazy");
        assert_eq!(spec.kind, ConnectionType::Mysql);
        assert!(spec.is_url);
        assert_eq!(spec.host.as_deref(), Some("localhost"));
        assert_eq!(spec.port, Some(3306));
        assert_eq!(spec.user.as_deref(), Some("user"));
        assert_eq!(spec.pass.as_deref(), Some("pass"));
        assert_eq!(spec.db_name.as_deref(), Some("lazy"));

        let spec = analyze_connection("postgres://db.azure.com:5432/prod");
        assert_eq!(spec.kind, ConnectionType::Postgres);
        assert_eq!(spec.host.as_deref(), Some("db.azure.com"));

        let spec = analyze_connection("mongodb://127.0.0.1:27017");
        assert_eq!(spec.kind, ConnectionType::Mongo);
        assert_eq!(spec.port, Some(27017));
        assert_eq!(spec.db_name, None);
    }

    #[test]
    fn parsea_uri_real_de_postgres_clevercloud() {
        // URI real de CleverCloud (producción): host con subdominio largo,
        // user/pass con chars raros, db en el path.
        let uri = "postgresql://uo8h6cfqdm4u5xv6lfqq:dsJBQr44561wnu9YizPLTeP1GFh0eO@\
                   bh4bhaewrpd8qqcreso5-postgresql.services.clever-cloud.com:5432/\
                   bh4bhaewrpd8qqcreso5";
        let spec = analyze_connection(uri);
        assert_eq!(spec.kind, ConnectionType::Postgres);
        assert_eq!(
            spec.host.as_deref(),
            Some("bh4bhaewrpd8qqcreso5-postgresql.services.clever-cloud.com")
        );
        assert_eq!(spec.port, Some(5432));
        assert_eq!(spec.user.as_deref(), Some("uo8h6cfqdm4u5xv6lfqq"));
        assert_eq!(spec.pass.as_deref(), Some("dsJBQr44561wnu9YizPLTeP1GFh0eO"));
        assert_eq!(spec.db_name.as_deref(), Some("bh4bhaewrpd8qqcreso5"));
    }

    #[test]
    fn parsea_uri_real_de_mysql_clevercloud() {
        let uri = "mysql://uytqyazji4f973j8:9IBrZmMMev5piMZREJbs@\
                   boe4cnhqy6qn2aqhbwxj-mysql.services.clever-cloud.com:3306/\
                   boe4cnhqy6qn2aqhbwxj";
        let spec = analyze_connection(uri);
        assert_eq!(spec.kind, ConnectionType::Mysql);
        assert_eq!(
            spec.host.as_deref(),
            Some("boe4cnhqy6qn2aqhbwxj-mysql.services.clever-cloud.com")
        );
        assert_eq!(spec.port, Some(3306));
        assert_eq!(spec.user.as_deref(), Some("uytqyazji4f973j8"));
        assert_eq!(spec.pass.as_deref(), Some("9IBrZmMMev5piMZREJbs"));
        assert_eq!(spec.db_name.as_deref(), Some("boe4cnhqy6qn2aqhbwxj"));
    }

    #[test]
    fn password_con_arroba_y_dos_puntos() {
        // La password puede contener `@` y `:` — se usa el ÚLTIMO `@` para
        // separar credenciales y el PRIMER `:` de las credenciales para user.
        let spec = analyze_connection("mysql://user:pa@ss:xx@localhost:3306/db");
        assert_eq!(spec.user.as_deref(), Some("user"));
        assert_eq!(spec.pass.as_deref(), Some("pa@ss:xx"));
        assert_eq!(spec.host.as_deref(), Some("localhost"));
        assert_eq!(spec.port, Some(3306));
        assert_eq!(spec.db_name.as_deref(), Some("db"));
    }

    #[test]
    fn puerto_por_defecto_cuando_no_viene() {
        let spec = analyze_connection("mysql://localhost/lazy");
        assert_eq!(spec.kind, ConnectionType::Mysql);
        assert_eq!(spec.port, Some(3306));
    }

    #[test]
    fn detecta_paths_por_extension() {
        assert_eq!(analyze_connection("/tmp/x.db").kind, ConnectionType::Sqlite);
        assert_eq!(analyze_connection("data.sqlite3").kind, ConnectionType::Sqlite);
        assert_eq!(analyze_connection("/mnt/datos/base.duckdb").kind, ConnectionType::Duckdb);
        assert_eq!(analyze_connection("/tmp/datos.csv").kind, ConnectionType::File);
        assert_eq!(analyze_connection("x.geojson").kind, ConnectionType::File);
        assert_eq!(analyze_connection("/mnt/x.gpkg").kind, ConnectionType::File);
    }

    #[test]
    fn urls_scheme_de_archivo() {
        let spec = analyze_connection("sqlite:///tmp/x.db");
        assert_eq!(spec.kind, ConnectionType::Sqlite);
        assert!(spec.is_url);
        assert_eq!(spec.file_path.as_deref(), Some("/tmp/x.db"));

        let spec = analyze_connection("duckdb:///tmp/x.duckdb");
        assert_eq!(spec.kind, ConnectionType::Duckdb);
    }

    #[test]
    fn path_absoluto_sin_extension_es_sqlite() {
        assert_eq!(analyze_connection("/tmp/mi_base").kind, ConnectionType::Sqlite);
    }

    #[test]
    fn vacio_y_desconocido() {
        assert_eq!(analyze_connection("").kind, ConnectionType::Unknown);
        assert_eq!(analyze_connection("hola mundo").kind, ConnectionType::Unknown);
    }

    #[test]
    fn display_legible() {
        let spec = analyze_connection("mysql://user:pass@localhost:3306/lazy");
        assert_eq!(spec.display(), "MySQL · localhost:3306/lazy");
    }
}
