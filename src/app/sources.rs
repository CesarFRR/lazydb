//! Panel Fuentes: builder de la lista de fuentes + helpers de formato +
//! probes de salud. Extraído del monolito `controller.rs` (Fase 1 del
//! refactor, validado por lazygit/gitui/ratatui: cada dominio = su archivo).

use std::collections::{HashMap, HashSet};

use crate::app::controller::SourceTab;
use crate::storage;

/// Clasificación tipada de una fuente: archivo local, servicio local o
/// servicio remoto. Reemplaza la heurística de strings que confundía
/// `sqlite://` con online y ocultaba las URLs de localhost del tab Local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceKind {
    /// Archivo local: `.db`, `.sqlite`, `.duckdb` o URL `sqlite://`.
    File,
    /// Servicio en la propia máquina: `mysql://localhost/...`, `[::1]`, etc.
    Localhost,
    /// Servicio remoto: `http(s)://`, `ssh://` o DB URL con host no local.
    Online,
}

pub fn source_kind(value: &str) -> SourceKind {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ssh://")
    {
        return SourceKind::Online;
    }
    if lower.starts_with("mysql://")
        || lower.starts_with("postgres://")
        || lower.starts_with("postgresql://")
        || lower.starts_with("mongodb://")
    {
        let host = url_host(&lower);
        let is_local =
            host.is_none_or(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]" || h == "::1");
        return if is_local { SourceKind::Localhost } else { SourceKind::Online };
    }
    // `sqlite://`, rutas relativas/absolutas y todo lo demás: archivo local
    SourceKind::File
}

/// Host de una URL de conexión: `mysql://user:pass@127.0.0.1:3306/db` →
/// `127.0.0.1`. Tolera IPv6 entre corchetes (`[::1]`).
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url.split("://").nth(1)?;
    let before_slash = rest.split('/').next()?;
    let host_port = before_slash.rsplit('@').next()?;
    host_port.find(']').map_or_else(
        || Some(host_port.split(':').next().unwrap_or(host_port)),
        |bracket_end| Some(&host_port[..=bracket_end]),
    )
}

/// Host y puerto de una URL de conexión: `mysql://user:pass@db:3306/x` →
/// `("db", 3306)`. Puertos por defecto: `MySQL` 3306, `PostgreSQL` 5432,
/// `http` 80, `https` 443. Tolera IPv6 entre corchetes.
pub fn source_host_port(url: &str) -> Option<(String, u16)> {
    let lower = url.to_ascii_lowercase();
    let default_port = if lower.starts_with("mysql://") {
        3306
    } else if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        5432
    } else if lower.starts_with("mongodb://") {
        27017
    } else if lower.starts_with("http://") {
        80
    } else if lower.starts_with("https://") {
        443
    } else {
        return None;
    };

    let rest = url.split("://").nth(1)?;
    let before_slash = rest.split('/').next()?;
    let host_port = before_slash.rsplit('@').next()?;

    let (host, port) = host_port.rfind(':').map_or((host_port, default_port), |colon| {
        // "host:no-numérico" → puerto por defecto
        host_port[colon + 1..]
            .parse::<u16>()
            .map_or((host_port, default_port), |p| (&host_port[..colon], p))
    });
    Some((host.trim_matches(['[', ']']).to_string(), port))
}

/// Máximo de probes de salud en paralelo. `probe_source` hace DNS + TCP
/// síncrono (hasta 2s por fuente); con el runtime por defecto (workers =
/// CPUs), un scroll rápido podía agotar los workers y frenar las queries.
pub const PROBE_MAX_CONCURRENT: usize = 4;

/// Probe de salud de una fuente (filosofía culling: se invoca solo sobre la
/// fuente seleccionada, en segundo plano, y el resultado se cachea):
/// - URLs (`mysql://`, `postgres://`, `http(s)://`…): conexión TCP al
///   host:puerto con timeout de 2s (el servicio existe aunque luego falle la
///   autenticación).
/// - Archivos: comprobación de que existen y son legibles (no se abre la DB).
pub fn probe_source(path: &str) -> bool {
    match source_kind(path) {
        SourceKind::File => {
            let file = path
                .strip_prefix("sqlite://")
                .or_else(|| path.strip_prefix("duckdb://"))
                .unwrap_or(path);
            std::fs::metadata(file).is_ok_and(|meta| meta.is_file())
        }
        SourceKind::Localhost | SourceKind::Online => {
            let Some((host, port)) = source_host_port(path) else {
                return false;
            };
            // Hostname → resolución DNS bloqueante (aceptable: va en spawn_blocking)
            let addr: Option<std::net::SocketAddr> = host
                .parse::<std::net::IpAddr>()
                .map(|ip| std::net::SocketAddr::new(ip, port))
                .ok()
                .or_else(|| {
                    use std::net::ToSocketAddrs;
                    (host.as_str(), port).to_socket_addrs().ok()?.next()
                });
            let Some(addr) = addr else { return false };
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2)).is_ok()
        }
    }
}

/// Marca de tipo de base de datos para el panel Fuentes (1 carácter).
/// - `▣` `SQLite` (incluye URLs `sqlite://`) · `D` `DuckDB` · `M` `MySQL` ·
///   `P` `PostgreSQL` · `⊙` endpoint genérico (`http`/`https`/`ssh`)
pub fn db_type_mark(value: &str) -> char {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        'P'
    } else if lower.starts_with("mysql://") {
        'M'
    } else if lower.starts_with("mongodb://") {
        'N'
    } else if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ssh://")
    {
        '⊙'
    } else {
        let ext = std::path::Path::new(value).extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext.to_ascii_lowercase().as_str() {
            "duckdb" | "ddb" => 'D',
            "csv" => 'C',
            "tsv" => 'T',
            "parquet" | "pq" => 'P',
            "json" | "jsonl" | "ndjson" => 'J',
            "geojson" | "gpkg" => 'G',
            _ => '▣',
        }
    }
}

// ── formato de items del panel Fuentes ─────────────────────────────────
// Cada item es un string plano con marcas que el render colorea:
//   sección:   "\u{1}LABEL"          (marcador interno, no visible)
//   entry:     [● ][✗ ]<★|▣|D|M|P|N|⊙|C|T|J|G ><texto>
//                     ● = conectada, ✗ = con problemas (sin marca = bien),
//                     ★ = favorito, ▣ = sqlite, D = duckdb, M = mysql,
//                     P = postgres, N = mongodb, ⊙ = endpoint genérico,
//                     C = csv, T = tsv, J = json(jsonl), G = geojson/gpkg
// Los favoritos van al inicio de la lista sin sección propia (la ★ basta).
// Los favoritos usan "name => path"; el resto muestra el path directo.

/// Marcador interno de sección (SOH): nunca se renderiza tal cual.
pub const SOURCE_SECTION_MARK: char = '\u{1}';

pub fn source_section(label: &str) -> String {
    format!("{SOURCE_SECTION_MARK}{label}")
}

pub fn is_source_section(item: &str) -> bool {
    item.starts_with(SOURCE_SECTION_MARK)
}

/// Quita las credenciales (`user:pass@`) de una URL y devuelve el user.
/// Útil para el prompt de contraseña: no mostrar la password de nuevo y
/// sugerir el user que la URL ya traía. Acepta `mysql://`, `postgres://` y
/// `postgresql://` (el parámetro `scheme` cubre la variante canónica).
#[cfg(any(feature = "mysql", feature = "postgres"))]
pub fn strip_url_credentials(url: &str) -> (String, Option<String>) {
    // Encuentra `scheme://` o `scheme+algo://` (p.ej. postgresql://)
    let Some(at) = url.find("://") else {
        return (url.to_string(), None);
    };
    let (scheme_part, rest) = url.split_at(at + 3);
    let Some(at_mark) = rest.rfind('@') else {
        return (url.to_string(), None);
    };
    let creds = &rest[..at_mark];
    let host = &rest[at_mark + 1..];
    let user = creds.split_once(':').map_or(Some(creds), |(u, _)| Some(u)).map(ToString::to_string);
    (format!("{scheme_part}{host}"), user)
}

/// ¿La URL `mysql://`, `postgres://` o `mongodb://` incluye una base de datos
/// explícita (`.../bd`)? Las URLs de `scan_local_servers` llegan sin BD: son
/// servidores. Solo el flujo de servidores remotos (mysql/postgres/mongo) la usa.
#[cfg(any(feature = "mysql", feature = "postgres", feature = "mongodb"))]
pub fn server_url_has_database(url: &str) -> bool {
    let rest = url
        .strip_prefix("mysql://")
        .or_else(|| url.strip_prefix("postgres://"))
        .or_else(|| url.strip_prefix("postgresql://"))
        .or_else(|| url.strip_prefix("mongodb://"))
        .unwrap_or_default();
    // `host`, `host:puerto`, `user:pass@host:puerto` → sin `/` = sin BD.
    // Con `/bd` → hay base.
    rest.contains('/') && !rest.ends_with('/')
}

/// ¿Es una URL remota (`mysql://`, `postgres://`, `postgresql://`, `mongodb://`)?
pub fn is_remote_url(url: &str) -> bool {
    url.starts_with("mysql://")
        || url.starts_with("postgres://")
        || url.starts_with("postgresql://")
        || url.starts_with("mongodb://")
}

/// Inserta `user:pass@` tras el scheme de la URL (para reconectar con las
/// credenciales del keyring). Devuelve la URL con credenciales.
pub fn inject_credentials(url: &str, scheme: &str, user: &str, pass: &str) -> String {
    let prefix = format!("{scheme}://");
    url.strip_prefix(&prefix)
        .map_or_else(|| url.to_string(), |rest| format!("{prefix}{user}:{pass}@{rest}"))
}

/// Quita las marcas decorativas (● ★ ▣ ⊙, combinables: "● ★ x") de un item de
/// Fuentes y devuelve el dato real (path o "name => path" para favoritos).
pub fn strip_source_marks(mut item: &str) -> &str {
    loop {
        let mut stripped = false;
        for mark in ["● ", "★ ", "✗ ", "▣ ", "D ", "M ", "P ", "N ", "⊙ ", "C ", "T ", "J ", "G "]
        {
            if let Some(rest) = item.strip_prefix(mark) {
                item = rest;
                stripped = true;
            }
        }
        if !stripped {
            return item;
        }
    }
}

/// Item "resumen" del panel Fuentes cuando NO está enfocado (filosofía lazy,
/// como lazydocker muestra el contenedor seleccionado). Prioridad:
/// 1. La DB conectada (item con `● `) — el dato más útil del panel.
/// 2. La fuente bajo el cursor (si es un entry real).
/// 3. El primer entry de la lista.
///
/// Devuelve como máximo 1 item; vacío si no hay nada que resumir. Nunca
/// devuelve secciones (`\u{1}...`), placeholders ni acciones fijas.
pub fn source_summary(items: &[String], selected_idx: usize) -> Vec<&str> {
    let is_action = |s: &str| s == "Abrir sakila.db" || s == "Buscar archivo .db";

    // 1. DB conectada
    if let Some(connected) = items.iter().find(|s| s.starts_with("● ")) {
        return vec![connected];
    }
    // 2. Fuente bajo el cursor
    if let Some(sel) = items.get(selected_idx)
        && !is_source_section(sel)
        && *sel != "<sin entradas>"
        && !is_action(sel)
    {
        return vec![sel];
    }
    // 3. Primer entry real de la lista
    for item in items {
        if !is_source_section(item) && *item != "<sin entradas>" && !is_action(item) {
            return vec![item];
        }
    }
    Vec::new()
}

/// Extrae el path real de un item de Fuentes (con o sin marcas).
pub fn source_path_of(item: &str) -> &str {
    let clean = strip_source_marks(item);
    clean.split_once(" => ").map_or(clean, |(_, path)| path)
}

/// Filtro de visibilidad de una fuente según el tab activo.
#[derive(Clone, Copy)]
pub enum SourceFilter {
    All,
    Local,
    Online,
}

impl SourceFilter {
    fn passes(self, path: &str) -> bool {
        match self {
            Self::All => true,
            // Local = archivos + servicios de localhost (mysql://localhost…)
            Self::Local => matches!(source_kind(path), SourceKind::File | SourceKind::Localhost),
            Self::Online => source_kind(path) == SourceKind::Online,
        }
    }
}

/// Construye la lista del panel Fuentes por secciones: FAVORITOS, RECIENTES
/// y ARCHIVOS (./), con marcas de tipo, de salud y de DB conectada.
pub struct SourceList<'a> {
    state: &'a storage::AppState,
    connected: Option<&'a str>,
    health: &'a HashMap<String, bool>,
    out: Vec<String>,
    seen: HashSet<String>,
    sections: HashSet<String>,
}

impl SourceList<'_> {
    pub fn section(&mut self, label: &str) {
        if self.sections.insert(label.to_string()) {
            self.out.push(source_section(label));
        }
    }

    pub fn entry(&mut self, path: &str, display: Option<&str>) {
        // Choke point de normalización: los favoritos/recientes guardados por
        // versiones antiguas pueden estar en relativo; aquí todos se comparan
        // y muestran en canónico (absoluto), para que el `seen` deduplique y
        // la marca ● coincida con `connected`.
        let path = crate::paths::normalize_path(path);
        if !self.seen.insert(path.clone()) {
            return;
        }
        let is_fav = self.state.favorites.values().any(|v| v == &path);
        let prefix = if is_fav { "★ ".to_string() } else { format!("{} ", db_type_mark(&path)) };
        // Solo se marca el error: sin marca = la fuente está bien (no hay ✓).
        let health_mark = if self.health.get(&path) == Some(&false) { "✗ " } else { "" };
        let mark = format!(
            "{}{}",
            if self.connected == Some(path.as_str()) { "● " } else { "" },
            health_mark
        );
        match display {
            Some(name) => self.out.push(format!(
                "{mark}{prefix}{name} => {}",
                crate::security::strip_credentials(&path)
            )),
            None => self
                .out
                .push(format!("{mark}{prefix}{}", crate::security::strip_credentials(&path))),
        }
    }

    pub fn add_favs(&mut self, filter: SourceFilter) {
        let mut favs: Vec<(String, String)> = self
            .state
            .favorites
            .iter()
            .filter(|(_, p)| filter.passes(p))
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect();
        favs.sort_by(|a, b| a.0.cmp(&b.0));
        // Sin sección propia: la ★ ya identifica al favorito; van primero.
        for (name, path) in &favs {
            self.entry(path, Some(name));
        }
    }

    pub fn add_recents(&mut self, filter: SourceFilter) {
        let is_fav = |path: &str| self.state.favorites.values().any(|v| v == path);
        let recents: Vec<String> =
            self.state.recents.iter().filter(|p| filter.passes(p) && !is_fav(p)).cloned().collect();
        if recents.is_empty() {
            return;
        }
        self.section("RECIENTES");
        for recent in &recents {
            self.entry(recent, None);
        }
    }

    pub fn add_detected(&mut self, detected_servers: &[String], cwd_databases: &[String]) {
        // Servidores SQL locales detectados por puerto (escaneo cacheable)
        if !detected_servers.is_empty() {
            self.section("SERVIDORES LOCALES");
            for server in detected_servers {
                if !self.seen.contains(server) {
                    self.entry(server, None);
                }
            }
        }

        // DBs SQLite de la carpeta actual (donde se ejecuta lazydb). El
        // escaneo se cachea en `App::new` (I/O de filesystem bloqueante).
        let fresh: Vec<String> =
            cwd_databases.iter().filter(|p| !self.seen.contains(*p)).cloned().collect();
        if fresh.is_empty() {
            return;
        }
        self.section("ARCHIVOS (./)");
        for db in &fresh {
            self.entry(db, None);
        }
    }

    pub fn finish(mut self, source_tab: SourceTab) -> Vec<String> {
        if self.out.is_empty() {
            self.out.push("<sin entradas>".to_string());
        }
        // Acciones fijas: solo tienen sentido junto a fuentes locales
        if matches!(source_tab, SourceTab::All | SourceTab::Local) {
            self.out.push("Buscar archivo .db".to_string());
            self.out.push("Abrir sakila.db".to_string());
        }
        self.out
    }
}

/// Escanea el directorio de trabajo actual (donde se ejecuta `cargo run` /
/// lazydb) buscando archivos de base de datos locales: `*.db`, `*.sqlite`,
/// `*.sqlite3` (`SQLite`), `*.duckdb`, `*.ddb` (`DuckDB`) y archivos de datos
/// (`*.csv`, `*.tsv`, `*.parquet`, `*.json`, `*.jsonl`, `*.geojson`,
/// `*.gpkg`). Devuelve los paths completos ordenados alfabéticamente.
pub fn scan_cwd_databases() -> Vec<String> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&cwd) else {
        return Vec::new();
    };
    let mut dbs: Vec<String> = entries
        .flatten()
        .filter(|e| {
            let path = e.path();
            path.is_file()
                && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "db" | "sqlite"
                            | "sqlite3"
                            | "duckdb"
                            | "ddb"
                            | "csv"
                            | "tsv"
                            | "parquet"
                            | "pq"
                            | "json"
                            | "jsonl"
                            | "ndjson"
                            | "geojson"
                            | "gpkg"
                    )
                })
        })
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .collect();
    dbs.sort();
    dbs
}
pub fn build_sources(
    state: &storage::AppState,
    source_tab: SourceTab,
    connected: Option<&str>,
    health: &HashMap<String, bool>,
    detected_servers: &[String],
    cwd_databases: &[String],
) -> Vec<String> {
    let mut list = SourceList {
        state,
        connected,
        health,
        out: Vec::new(),
        seen: HashSet::new(),
        sections: HashSet::new(),
    };

    match source_tab {
        SourceTab::All => {
            list.add_favs(SourceFilter::All);
            list.add_recents(SourceFilter::All);
            list.add_detected(detected_servers, cwd_databases);
        }
        SourceTab::Local => {
            list.add_favs(SourceFilter::Local);
            list.add_recents(SourceFilter::Local);
            list.add_detected(detected_servers, cwd_databases);
        }
        SourceTab::Online => {
            list.add_favs(SourceFilter::Online);
            list.add_recents(SourceFilter::Online);
        }
    }

    list.finish(source_tab)
}
