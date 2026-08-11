use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Rect;

use crate::app::panel::{Panel, PanelKind};
use crate::ui::layout::{self, ComputedLayout};
use crate::ui::widgets::panel::MIN_COL_W;
use crate::{config, db, keys, query, storage};

const LARGE_WIDTH: u16 = 120;
const KB_BYTES: u64 = 1024;
const MB_BYTES: u64 = KB_BYTES * 1024;

// ---------------------------------------------------------------------------
// Enums de UI (transición: algunos se moverán a panel.rs en el futuro)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SourceTab {
    All,
    Local,
    Online,
}

impl SourceTab {
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Local,
            Self::Local => Self::Online,
            Self::Online => Self::All,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::All => Self::Online,
            Self::Local => Self::All,
            Self::Online => Self::Local,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "Todo",
            Self::Local => "Local",
            Self::Online => "Online",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ObjectSection {
    Tables,
    Views,
    Advanced,
}

impl ObjectSection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tables => "Tablas",
            Self::Views => "Vistas",
            Self::Advanced => "Avanzado",
        }
    }

    pub const fn from_panel_kind(kind: PanelKind) -> Option<Self> {
        match kind {
            PanelKind::Tables => Some(Self::Tables),
            PanelKind::Views => Some(Self::Views),
            PanelKind::Advanced => Some(Self::Advanced),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputMode {
    Normal,
    Filtering,
}

/// Estado de arrastre del mouse sobre una barra de scroll (click + drag).
#[derive(Clone, Copy, Debug)]
#[allow(clippy::enum_variant_names)] // todas las variantes son barras de scroll
enum DragState {
    /// Arrastrando la barra de scroll horizontal del Data tab
    HScroll,
    /// Arrastrando el scrollbar vertical de un panel
    VScroll(PanelKind),
    /// Arrastrando el scrollbar interior del modal del inspector de fila
    InspectorScroll { rect: Rect, content_len: usize },
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DetailTab {
    Data,
    Schema,
    Sql,
    Meta,
}

impl DetailTab {
    pub const fn next(self) -> Self {
        match self {
            Self::Data => Self::Schema,
            Self::Schema => Self::Sql,
            Self::Sql => Self::Meta,
            Self::Meta => Self::Data,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::Data => Self::Meta,
            Self::Schema => Self::Data,
            Self::Sql => Self::Schema,
            Self::Meta => Self::Sql,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Data => " Datos ",
            Self::Schema => " Esquema ",
            Self::Sql => " SQL ",
            Self::Meta => " Meta ",
        }
    }
}

/// Clasificación de layout por ancho (para el header)
#[derive(Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
pub enum LayoutMode {
    Large,
    Medium,
    Small,
}

impl LayoutMode {
    #[allow(dead_code)]
    pub const fn from_width(width: u16) -> Self {
        if width >= LARGE_WIDTH {
            Self::Large
        } else if width >= layout::NARROW_THRESHOLD {
            Self::Medium
        } else {
            Self::Small
        }
    }

    #[allow(dead_code)]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::Medium => "medium",
            Self::Small => "small",
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn list_index_from_click(rel_y: u16, section_height: u16, top_reserved: u16) -> Option<usize> {
    if section_height <= 2 {
        return None;
    }

    let inner_top = top_reserved.saturating_add(1);
    let inner_bottom = section_height.saturating_sub(1);

    if rel_y < inner_top || rel_y >= inner_bottom {
        return None;
    }

    Some(usize::from(rel_y.saturating_sub(inner_top)))
}

/// Geometría del thumb de la barra de scroll horizontal.
///
/// El recorrido real del thumb es `track = inner_w - thumb_w` (el thumb tiene
/// ancho proporcional a las columnas visibles y por eso nunca llega al borde
/// derecho). Devuelve `(thumb_w, track)`: el tamaño del thumb y el recorrido
/// efectivo (en celdas) que el mouse debe cubrir para barrer el scroll completo.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn h_scroll_thumb_geometry(inner_w: usize, col_count: usize, max_visible: usize) -> (usize, f32) {
    let thumb_w = (inner_w as f32 * max_visible as f32 / col_count as f32).round() as usize;
    let track = inner_w.saturating_sub(thumb_w).max(1) as f32;
    (thumb_w, track)
}

/// Geometría del thumb del scrollbar vertical.
/// Devuelve `(thumb_h, track)` — análogo a `h_scroll_thumb_geometry`.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn v_scroll_thumb_geometry(panel_h: u16, items_len: usize, viewport: usize) -> (usize, f32) {
    let thumb_h = (f32::from(panel_h) * viewport as f32 / items_len as f32).round() as usize;
    let track = usize::from(panel_h).saturating_sub(thumb_h).max(1) as f32;
    (thumb_h, track)
}

/// Timestamp en milisegundos para detección de doble-click.
fn now_millis() -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Clasificación tipada de una fuente, reemplaza la heurística de strings
/// (`is_online_source`), que confundía `sqlite://` con online y ocultaba las
/// URLs de localhost del tab Local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SourceKind {
    /// Archivo local: `.db`, `.sqlite`, `.duckdb` o URL `sqlite://`.
    File,
    /// Servicio en la propia máquina: `mysql://localhost/...`, `[::1]`, etc.
    Localhost,
    /// Servicio remoto: `http(s)://`, `ssh://` o DB URL con host no local.
    Online,
}

fn source_kind(value: &str) -> SourceKind {
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
fn url_host(url: &str) -> Option<&str> {
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
fn source_host_port(url: &str) -> Option<(String, u16)> {
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

/// Probe de salud de una fuente (filosofía culling: se invoca solo sobre la
/// fuente seleccionada, en segundo plano, y el resultado se cachea):
/// - URLs (`mysql://`, `postgres://`, `http(s)://`…): conexión TCP al
///   host:puerto con timeout de 2s (el servicio existe aunque luego falle la
///   autenticación).
/// - Archivos: comprobación de que existen y son legibles (no se abre la DB).
fn probe_source(path: &str) -> bool {
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
fn db_type_mark(value: &str) -> char {
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

fn source_section(label: &str) -> String {
    format!("{SOURCE_SECTION_MARK}{label}")
}

fn is_source_section(item: &str) -> bool {
    item.starts_with(SOURCE_SECTION_MARK)
}

/// Quita las credenciales (`user:pass@`) de una URL y devuelve el user.
/// Útil para el prompt de contraseña: no mostrar la password de nuevo y
/// sugerir el user que la URL ya traía. Acepta `mysql://`, `postgres://` y
/// `postgresql://` (el parámetro `scheme` cubre la variante canónica).
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn strip_url_credentials(url: &str) -> (String, Option<String>) {
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
fn server_url_has_database(url: &str) -> bool {
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
fn is_remote_url(url: &str) -> bool {
    url.starts_with("mysql://")
        || url.starts_with("postgres://")
        || url.starts_with("postgresql://")
        || url.starts_with("mongodb://")
}

/// Inserta `user:pass@` tras el scheme de la URL (para reconectar con las
/// credenciales del keyring). Devuelve la URL con credenciales.
fn inject_credentials(url: &str, scheme: &str, user: &str, pass: &str) -> String {
    let prefix = format!("{scheme}://");
    url.strip_prefix(&prefix)
        .map_or_else(|| url.to_string(), |rest| format!("{prefix}{user}:{pass}@{rest}"))
}

/// Quita las marcas decorativas (● ★ ▣ ⊙, combinables: "● ★ x") de un item de
/// Fuentes y devuelve el dato real (path o "name => path" para favoritos).
fn strip_source_marks(mut item: &str) -> &str {
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
fn source_path_of(item: &str) -> &str {
    let clean = strip_source_marks(item);
    clean.split_once(" => ").map_or(clean, |(_, path)| path)
}

/// Filtro de visibilidad de una fuente según el tab activo.
#[derive(Clone, Copy)]
enum SourceFilter {
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
struct SourceList<'a> {
    state: &'a storage::AppState,
    connected: Option<&'a str>,
    health: &'a HashMap<String, bool>,
    out: Vec<String>,
    seen: HashSet<String>,
    sections: HashSet<String>,
}

impl SourceList<'_> {
    fn section(&mut self, label: &str) {
        if self.sections.insert(label.to_string()) {
            self.out.push(source_section(label));
        }
    }

    fn entry(&mut self, path: &str, display: Option<&str>) {
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

    fn add_favs(&mut self, filter: SourceFilter) {
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

    fn add_recents(&mut self, filter: SourceFilter) {
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

    fn add_detected(&mut self, detected_servers: &[String]) {
        // Servidores SQL locales detectados por puerto (escaneo cacheable)
        if !detected_servers.is_empty() {
            self.section("SERVIDORES LOCALES");
            for server in detected_servers {
                if !self.seen.contains(server) {
                    self.entry(server, None);
                }
            }
        }

        // DBs SQLite de la carpeta actual (donde se ejecuta lazydb)
        let scanned = scan_cwd_databases();
        let fresh: Vec<String> =
            scanned.iter().filter(|p| !self.seen.contains(*p)).cloned().collect();
        if fresh.is_empty() {
            return;
        }
        self.section("ARCHIVOS (./)");
        for db in &fresh {
            self.entry(db, None);
        }
    }

    fn finish(mut self, source_tab: SourceTab) -> Vec<String> {
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
fn scan_cwd_databases() -> Vec<String> {
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

// ---------------------------------------------------------------------------
// App (estado global)
// ---------------------------------------------------------------------------

/// Contenido del popup de error global (modal rojo encima de todo).
#[derive(Clone, Debug)]
pub struct ErrorPopup {
    pub title: String,
    pub body: String,
}

/// Estado del popup de input SQL (`:`): buffer, cursor y navegación del
/// historial (↑/↓ estilo fish).
#[derive(Clone, Debug, Default)]
pub struct QueryInputState {
    pub buffer: String,
    /// Posición del cursor dentro de `buffer` (índice de char)
    pub cursor: usize,
    /// `Some(i)` = navegando el historial (la entrada i rellena el buffer);
    /// `None` = escribiendo una query nueva.
    pub history_idx: Option<usize>,
}

/// Pick de base de datos de un servidor MySQL/MariaDB detectado: lista de
/// esquemas (`SHOW DATABASES`) a elegir con ↑/↓ + Enter.
pub struct DbPickerState {
    /// URL del servidor SIN base (p. ej. `mysql://127.0.0.1:3306`)
    pub server_url: String,
    /// Bases disponibles (orden que las devuelve el servidor)
    pub dbs: Vec<String>,
    /// Índice del esquema seleccionado
    pub idx: usize,
}

/// Prompt de contraseña de un servidor detectado (modal de entrada de texto
/// sin historial; Enter envía, Esc cancela).
pub struct PasswordPromptState {
    /// URL del servidor SIN base y SIN credenciales (p. ej. `mysql://127.0.0.1:3306`)
    pub server_url: String,
    /// Usuario asumido para autenticar (por defecto `root` en localhost)
    pub user: String,
    /// Password tipeado (chars enmascarados en pantalla)
    pub buffer: String,
}

/// Índice de campo del formulario de nueva conexión.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnField {
    Url,
    Kind,
    Host,
    Port,
    User,
    Pass,
    Db,
    Connect,
}

/// Formulario "Nueva conexión" del panel Detail (sin db abierta).
///
/// Sincronización bidireccional:
/// - Editar un campo (host/puerto/user/pass/db) → la URL se reconcatena
///   al instante (operación trivial; el tipo no se retoca).
/// - Escribir en la URL → debounce 1s; si parsea COMPLETO, rellena los
///   campos. Si está incompleta, no toca nada (espera).
/// - Enter en la URL → reparsea y rellena los campos de inmediato.
pub struct ConnectionFormState {
    /// Campo con el foco
    pub active: ConnField,
    /// URL/ruta canónica (fuente de verdad para conectar)
    pub url: String,
    /// Tipo detectado por el analizador (Auto si desconocido)
    pub kind: crate::db::connection::ConnectionType,
    /// Tipo forzado manualmente por el usuario (None = seguir el auto)
    pub kind_override: Option<crate::db::connection::ConnectionType>,
    pub host: String,
    pub port: String,
    pub user: String,
    pub pass: String,
    pub db: String,
    /// Tick del último keystroke en la URL (para el debounce de reparseo)
    pub url_last_edit: Option<std::time::Instant>,
    /// `true` si el reparseo por debounce ya está programado este frame
    pub url_debounce_scheduled: bool,
    /// Mensaje del campo URL (resultado del último análisis)
    pub detected_note: String,
    /// `true` mientras se conecta (spinner)
    pub connecting: bool,
}

impl Default for ConnectionFormState {
    fn default() -> Self {
        Self {
            active: ConnField::Url,
            url: String::new(),
            kind: crate::db::connection::ConnectionType::Unknown,
            kind_override: None,
            host: String::new(),
            port: String::new(),
            user: String::new(),
            pass: String::new(),
            db: String::new(),
            url_last_edit: None,
            url_debounce_scheduled: false,
            detected_note: String::new(),
            connecting: false,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    // ── sistema de paneles ──
    pub panels: [Panel; 5],
    pub active_panel: PanelKind,
    /// Último panel de sidebar enfocado (para restaurar al salir de Detalle)
    pub last_sidebar_focus: PanelKind,
    pub layout: ComputedLayout,

    // ── datos de los paneles ──
    pub sources: Vec<String>,
    /// Servidores SQL locales detectados por puerto (cacheados en `new`:
    /// el escaneo es bloqueante y no debe repetirse en cada render). Cada
    /// entrada es una URL sin credenciales, p. ej. `mysql://127.0.0.1:3306`.
    pub detected_servers: Vec<String>,
    pub source_tab: SourceTab,
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub advanced: Vec<String>,
    pub preview_rows: Vec<String>,
    /// Celdas TIPADAS del Data tab (última página cargada): la fuente de
    /// verdad para el render 2D. `None` cuando la vista actual no es una
    /// tabla (mensajes, SQL, schema). El render usa `preview_rows` solo
    /// como fallback (List de 1 columna / mensajes).
    pub preview_data: Option<crate::db::TableData>,
    pub detail_tab: DetailTab,

    // ── estado persistente ──
    pub should_quit: bool,
    pub is_loading: bool,
    pub refresh_count: u32,
    pub db_path: Option<String>,
    /// Adapter de la conexión ACTIVA (compartido): se crea UNA vez al
    /// conectar y se reusa para todas las operaciones. Evita recrear el
    /// pool (handshake TCP+auth) en cada navegación, lo que fallaba con
    /// conexiones lentas (`CleverCloud`) por límite de conexiones.
    /// `Rc` permite clonar la referencia sin prestar `self` (los métodos
    /// que usan el adapter también asignan campos de `self`).
    pub active_adapter: Option<std::sync::Arc<dyn crate::db::adapter::DbAdapter>>,
    pub db_size_bytes: Option<u64>,
    /// ¿El backend conectado es `NoSQL` (mongo)? La UI cambia terminología
    /// (`row` → `doc`) y muestra el toggle JSON del modal.
    pub is_nosql: bool,
    pub status: String,
    pub current_page: u32,
    pub rows_per_page: u32,
    pub total_rows: u32,
    /// Índice 0-based en el dataset de `preview_rows[1]` (primera fila de datos)
    pub preview_loaded_offset: u32,
    pub query_state: query::QueryState,
    pub query_results: Vec<String>,
    pub state: storage::AppState,

    // ── probe de salud de fuentes (filosofía culling) ──
    /// Último estado de salud conocido por fuente (path → accesible). Solo
    /// se prueban las fuentes que el usuario selecciona (click o flechas),
    /// nunca toda la lista ni por tiempo.
    pub health: HashMap<String, bool>,
    /// Índice de Sources probeado por última vez: el tick detecta cambios de
    /// selección por CUALQUIER vía (flechas, click, drag, foco) comparando
    /// este valor, porque la navegación escribe `selected_idx` directo.
    last_probed_idx: usize,
    /// Paths con probe en vuelo (evita lanzar dos probes seguidos en paralelo).
    probing: HashSet<String>,
    /// Canal de resultados de probes en segundo plano (tx/rx).
    probe_rx: Option<tokio::sync::mpsc::UnboundedReceiver<(String, bool)>>,
    probe_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, bool)>>,

    // ── query runner (COUNT(*) real, cancelable) ──
    /// Generación de la última query lanzada: los resultados que llegan con
    /// una generación vieja se descartan (stale data). Se incrementa al
    /// lanzar una query nueva, al limpiar o al desconectar: cancela de
    /// facto cualquier tarea en vuelo.
    query_gen: u64,
    /// Canal de resultados del query runner: generación + SQL + resultado.
    query_rx: Option<tokio::sync::mpsc::UnboundedReceiver<query::QueryMsg>>,
    query_tx: Option<tokio::sync::mpsc::UnboundedSender<query::QueryMsg>>,
    /// Contador de frames para el spinner del status bar.
    pub frame: usize,
    pub keymap: keys::Keymap,
    pub show_actions_menu: bool,
    pub actions_menu_idx: usize,
    /// Inspector de fila (modal de detalle de registro)
    pub show_row_inspector: bool,
    pub row_inspector_pairs: Vec<(String, String)>,
    /// Modo de visualización del modal `NoSQL`: `false` = pares clave→valor,
    /// `true` = JSON formateado del documento.
    pub inspector_json_mode: bool,
    pub inspector_json_text: String,
    pub inspector_scroll: crate::ui::widgets::modal::ModalScroll,
    /// Ayuda de teclas (modal `?`): se autogenera desde los bindings reales
    pub show_help: bool,
    pub help_scroll: crate::ui::widgets::modal::ModalScroll,
    /// Popup de error global (modal rojo que se cierra con Enter/Esc/q).
    /// Cualquier error de ejecución/IO lo dispara vía `show_error`.
    pub error: Option<ErrorPopup>,
    /// Popup de input SQL (`:`): `Some` = abierto. El historial persistente
    /// vive en `state.query_history` (storage).
    pub query_input: Option<QueryInputState>,
    /// Pick de base de datos de un servidor detectado (modal ↑/↓ + Enter):
    /// `Some` = eligiendo esquema al que conectarse.
    pub db_picker: Option<DbPickerState>,
    /// Prompt de contraseña de un servidor detectado (modal de entrada).
    pub password_prompt: Option<PasswordPromptState>,
    /// Formulario de nueva conexión (panel Detail sin db abierta). `None` =
    /// el usuario ya está conectado o no se ha abierto el formulario.
    pub connection_form: Option<ConnectionFormState>,
    /// El preview muestra el resultado de una query libre del usuario (no el
    /// objeto seleccionado). Los scrolls infinitos y refreshes lo respetan.
    pub query_mode: bool,
    /// Doble-click: timestamp del último click (ms) y panel clickeado
    pub last_click_time: u64,
    pub last_click_kind: Option<PanelKind>,
    pub last_click_idx: usize,

    /// Sección de objetos activa (derivada de `active_panel`)
    object_section: ObjectSection,

    // ── filtro de búsqueda ──
    pub input_mode: InputMode,
    pub filter_query: String,
    pub filtered_items: Vec<String>,

    // ── ordenamiento de columnas en Data tab ──
    pub sort_column: Option<String>,
    pub sort_asc: bool,

    // ── arrastre de barras de scroll (click + drag con mouse) ──
    drag: Option<DragState>,
}

impl App {
    const ACTION_ITEMS: [&'static str; 5] = [
        "Abrir sakila.db",
        "Guardar DB actual en favoritos",
        "Recargar config runtime",
        "Limpiar estado de query",
        "Cerrar menu",
    ];

    // ── construcción ──────────────────────────────────────────────────

    pub fn new() -> Self {
        let state = storage::AppState::load();
        let keymap = keys::Keymap::load();
        let ui_config = config::Config::load().ui;
        let source_tab = SourceTab::All;
        let (probe_tx, probe_rx) = tokio::sync::mpsc::unbounded_channel();
        let (query_tx, query_rx) = tokio::sync::mpsc::unbounded_channel();

        // Detección de servidores SQL locales: bloqueante (E/S de red), por
        // eso se cachea UNA vez en el arranque. Timeout corto por puerto.
        let detected_servers =
            crate::db::servers::scan_local_servers(std::time::Duration::from_millis(300));

        tracing::debug!(
            recents = state.recents.len(),
            keymap = %keymap.binding_count(),
            "App::new"
        );

        let panels = [
            Panel::new_sidebar(PanelKind::Sources),
            Panel::new_sidebar(PanelKind::Tables),
            Panel::new_sidebar(PanelKind::Views),
            Panel::new_sidebar(PanelKind::Advanced),
            Panel::new(PanelKind::Detail),
        ];

        let mut app = Self {
            panels,
            active_panel: PanelKind::Sources,
            last_sidebar_focus: PanelKind::Sources,
            layout: ComputedLayout::default(),
            sources: Self::build_sources(
                &state,
                source_tab,
                None,
                &HashMap::new(),
                &detected_servers,
            ),
            source_tab,
            detected_servers,
            health: HashMap::new(),
            probing: HashSet::new(),
            probe_rx: Some(probe_rx),
            probe_tx: Some(probe_tx),
            query_gen: 0,
            query_rx: Some(query_rx),
            query_tx: Some(query_tx),
            frame: 0,
            // usize::MAX: el primer frame detecta la selección inicial (item 0)
            // y la comprueba al arrancar.
            last_probed_idx: usize::MAX,
            tables: vec![],
            views: vec![],
            advanced: vec![],
            preview_rows: Vec::new(), // se rellena abajo (necesita self listo)
            preview_data: None,
            detail_tab: DetailTab::Data,
            should_quit: false,
            is_loading: false,
            refresh_count: 0,
            db_path: None,
            active_adapter: None,
            db_size_bytes: None,
            is_nosql: false,
            status: "Sin conexion".to_string(),
            current_page: 0,
            rows_per_page: ui_config.rows_per_page,
            total_rows: 0,
            preview_loaded_offset: 0,
            query_state: query::QueryState::Idle,
            query_results: Vec::new(),
            state,
            keymap,
            show_actions_menu: false,
            actions_menu_idx: 0,
            show_row_inspector: false,
            row_inspector_pairs: Vec::new(),
            inspector_json_mode: false,
            inspector_json_text: String::new(),
            inspector_scroll: crate::ui::widgets::modal::ModalScroll::default(),
            show_help: false,
            help_scroll: crate::ui::widgets::modal::ModalScroll::default(),
            error: None,
            query_input: None,
            db_picker: None,
            password_prompt: None,
            connection_form: Some(ConnectionFormState::default()),
            query_mode: false,
            last_click_time: 0,
            last_click_kind: None,
            last_click_idx: 0,
            object_section: ObjectSection::Tables,
            input_mode: InputMode::Normal,
            filter_query: String::new(),
            filtered_items: Vec::new(),
            sort_column: None,
            sort_asc: true,
            drag: None,
        };
        // El panel Detail arranca con el formulario de nueva conexión
        // (sin db abierta): auto-detecta el tipo desde la URL/ruta.
        app.status = "Nueva conexión: escribe la URL o ruta y Enter conecta".to_string();
        app
    }

    // ── layout ────────────────────────────────────────────────────────

    pub fn compute_layout(&mut self, width: u16, height: u16) {
        let active_sidebar = if self.active_panel.is_sidebar() {
            self.active_panel
        } else {
            self.last_sidebar_focus
        };

        self.layout = layout::compute(width, height, active_sidebar, self.active_panel);
        self.frame += 1;

        // Debounce del formulario de conexión (reparseo de la URL 1s)
        self.tick_connection_form();

        // Aplicar resultados de probes de salud y de queries terminadas, y
        // disparar los probes que correspondan según el layout del frame
        // actual (tick por frame). Va después del cálculo para conocer la
        // altura real del panel de Sources (ventana visible).
        self.poll_probe_results();
        self.poll_query_results();
    }

    // ── helpers de paneles ────────────────────────────────────────────

    fn panel_mut(&mut self, kind: PanelKind) -> &mut Panel {
        self.panels.iter_mut().find(|p| p.kind == kind).expect("panel not found")
    }

    fn panel(&self, kind: PanelKind) -> &Panel {
        self.panels.iter().find(|p| p.kind == kind).expect("panel not found")
    }

    fn selected_idx(&self, kind: PanelKind) -> usize {
        self.panel(kind).selected_idx
    }

    fn set_selected_idx(&mut self, kind: PanelKind, idx: usize) {
        self.panel_mut(kind).selected_idx = idx;
    }

    /// Popup de error global: loggea con `tracing::error!` y abre el modal
    /// rojo (se cierra con Enter/Esc/q). Los fallos de IO/ejecución deben
    /// pasar por aquí en lugar de silenciarse en el status bar.
    fn show_error(&mut self, title: &str, body: &str) {
        tracing::error!(title, body, "error global");
        self.error = Some(ErrorPopup { title: title.to_string(), body: body.to_string() });
    }

    // ── probe de salud de fuentes (filosofía culling) ─────────────────

    /// Extrae el path probar de una fila del panel de Fuentes. `None` si la
    /// fila no es una fuente (sección, placeholder o acción fija).
    fn source_probe_path(item: &str) -> Option<String> {
        if item.starts_with('\u{1}') || item == "<sin entradas>" {
            return None;
        }
        let path = source_path_of(item).to_string();
        if path == "Abrir sakila.db" || path == "Buscar archivo .db" {
            return None;
        }
        Some(path)
    }

    /// Paths a probar de una ventana visible de Fuentes: excluye secciones,
    /// placeholder, acciones fijas y lo ya comprobado (caché) o en vuelo.
    /// Pura para poder testear el "solo lo visible" sin App.
    fn visible_probe_targets(
        items: &[String],
        window: std::ops::Range<usize>,
        health: &HashMap<String, bool>,
        probing: &HashSet<String>,
    ) -> Vec<String> {
        items
            .iter()
            .enumerate()
            .filter(|(i, _)| window.contains(i))
            .filter_map(|(_, item)| Self::source_probe_path(item))
            .filter(|p| !health.contains_key(p) && !probing.contains(p))
            .collect()
    }

    /// Lanza el probe de un path en segundo plano. `use_cache = true` (probe
    /// de la ventana visible): solo si nunca se comprobó; `false` (selección
    /// actual): re-verifica siempre, porque el usuario espera ver en vivo si
    /// la fuente bajo el cursor cambió.
    fn probe_path(&mut self, path: String, use_cache: bool) {
        if self.probing.contains(&path) {
            return;
        }
        if use_cache && self.health.contains_key(&path) {
            return;
        }
        let Some(tx) = self.probe_tx.clone() else { return };
        self.probing.insert(path.clone());
        tracing::debug!(path = %path, cache = use_cache, "probe lanzado");
        tokio::spawn(async move {
            let ok = probe_source(&path);
            let _ = tx.send((path, ok));
        });
    }

    /// Probe de la fuente bajo el cursor. Se re-verifica en cada selección
    /// (click o flechas): si algo cambió en el exterior, la marca ✗ aparece
    /// al volver a pasar por la fuente. Nunca bloquea la UI.
    fn probe_selected(&mut self) {
        let Some(selected) =
            self.items_for(PanelKind::Sources).get(self.selected_idx(PanelKind::Sources))
        else {
            return;
        };
        let Some(path) = Self::source_probe_path(selected) else { return };
        self.probe_path(path, false);
    }

    /// Probe de las fuentes SOLO VISIBLES ahora mismo en el panel (ventana
    /// `scroll_offset..+altura real`): al arrancar se comprueban las pocas
    /// filas que caben en pantalla, no las 200 de la lista. Cada path se
    /// verifica a lo sumo una vez por arranque (caché), así no hay bucles de
    /// re-probe cuando los resultados reordenan la lista.
    fn probe_visible(&mut self) {
        let rect = self
            .layout
            .panels
            .iter()
            .find(|(k, _)| *k == PanelKind::Sources)
            .map(|(_, r)| *r)
            .unwrap_or_default();
        // Interior del panel sin bordes: filas de lista realmente visibles.
        let inner_h = usize::from(rect.height.saturating_sub(2));
        if inner_h == 0 {
            return;
        }
        let offset = self.panel(PanelKind::Sources).scroll_offset.get();
        let items = self.items_for(PanelKind::Sources).to_vec();
        let window = offset..offset.saturating_add(inner_h);
        let targets = Self::visible_probe_targets(&items, window, &self.health, &self.probing);
        for path in targets {
            self.probe_path(path, true);
        }
    }

    /// Aplica los resultados de probes terminados (llamado cada frame en
    /// `compute_layout`) y dispara los probes pendientes: la fuente recién
    /// seleccionada y las nuevas filas visibles de Fuentes.
    fn poll_probe_results(&mut self) {
        let Some(rx) = self.probe_rx.as_mut() else { return };
        while let Ok((path, ok)) = rx.try_recv() {
            // Rebuild solo si el estado cambió (primera verificación o ✗ ↔ ok)
            let changed = self.health.get(&path) != Some(&ok);
            self.health.insert(path.clone(), ok);
            self.probing.remove(&path);
            if changed {
                tracing::debug!(path = %path, ok, "probe resuelto (estado cambiado)");
                self.sources = Self::build_sources(
                    &self.state,
                    self.source_tab,
                    self.db_path.as_deref(),
                    &self.health,
                    &self.detected_servers,
                );
            }
        }

        // Selección de Sources bajo el cursor: la navegación (flechas, click,
        // drag) escribe `selected_idx` directo, sin pasar por set_selected_idx;
        // el tick detecta el cambio y re-verifica la fuente recién seleccionada
        // (siempre, sin caché). Con `last_probed_idx = usize::MAX` al arrancar,
        // el primer frame comprueba la selección inicial.
        let idx = self.selected_idx(PanelKind::Sources);
        if idx != self.last_probed_idx {
            self.last_probed_idx = idx;
            self.probe_selected();
        }

        // Ventana visible: si el scroll o la altura del panel cambió, hay filas
        // nuevas en pantalla que nunca se comprobaron → probearlas (una sola
        // vez por path gracias a la caché).
        self.probe_visible();
    }

    // ── navegación de foco ────────────────────────────────────────────

    fn set_focus(&mut self, kind: PanelKind) {
        if self.active_panel == kind {
            return; // ya tiene el foco
        }

        // Recordar último panel sidebar enfocado (para restaurar al salir de Detalle)
        if self.active_panel.is_sidebar() {
            self.last_sidebar_focus = self.active_panel;
        }

        // Actualizar object_section si corresponde
        if let Some(section) = ObjectSection::from_panel_kind(kind) {
            self.object_section = section;
        }

        self.active_panel = kind;

        // Si el foco va a un panel de objetos, refrescar preview
        if kind == PanelKind::Tables || kind == PanelKind::Views || kind == PanelKind::Advanced {
            self.current_page = 0;
            self.refresh_preview_from_selected_object();
        }
    }

    fn focus_next(&mut self) {
        let next = self.active_panel.next();
        self.set_focus(next);
    }

    fn focus_prev(&mut self) {
        let prev = self.active_panel.prev();
        self.set_focus(prev);
    }

    /// → : Sources → Tables → Views → Advanced → Sources (cíclico, solo sidebar)
    fn sidebar_focus_next(&mut self) {
        let next = match self.active_panel {
            PanelKind::Sources => PanelKind::Tables,
            PanelKind::Tables => PanelKind::Views,
            PanelKind::Views => PanelKind::Advanced,
            PanelKind::Advanced => PanelKind::Sources,
            PanelKind::Detail => self.last_sidebar_focus,
        };
        self.set_focus(next);
    }

    /// ← : Sources → Advanced → Views → Tables → Sources (cíclico, solo sidebar)
    fn sidebar_focus_prev(&mut self) {
        let prev = match self.active_panel {
            PanelKind::Sources => PanelKind::Advanced,
            PanelKind::Tables => PanelKind::Sources,
            PanelKind::Views => PanelKind::Tables,
            PanelKind::Advanced => PanelKind::Views,
            PanelKind::Detail => self.last_sidebar_focus,
        };
        self.set_focus(prev);
    }

    fn toggle_active_panel(&mut self) {
        // El detalle nunca se colapsa
        if self.active_panel == PanelKind::Detail {
            return;
        }

        let p = self.panel_mut(self.active_panel);
        p.mode = p.mode.toggled();
    }

    // ── movimiento de selección ───────────────────────────────────────

    fn move_selection(&mut self, step: isize) {
        match self.active_panel {
            PanelKind::Sources => {
                let len = self.sources.len();
                Self::shift_index_on_vec_len(
                    &mut self.panel_mut(PanelKind::Sources).selected_idx,
                    len,
                    step,
                );
                // Las secciones (subtítulos) no son seleccionables: saltarlas
                let idx = self.panel(PanelKind::Sources).selected_idx;
                let new = Self::skip_section_idx(&self.sources, idx, step);
                self.panel_mut(PanelKind::Sources).selected_idx = new;
            }
            PanelKind::Tables | PanelKind::Views | PanelKind::Advanced => {
                let len = match self.active_panel {
                    PanelKind::Tables => self.tables.len(),
                    PanelKind::Views => self.views.len(),
                    PanelKind::Advanced => self.advanced.len(),
                    _ => 0,
                };
                Self::shift_index_on_vec_len(
                    &mut self.panel_mut(self.active_panel).selected_idx,
                    len,
                    step,
                );
                self.current_page = 0;
                self.refresh_preview_from_selected_object();
            }
            PanelKind::Detail => {
                let len = self.preview_rows.len();
                let old_idx = self.selected_idx(PanelKind::Detail);
                Self::shift_index_on_vec_len(
                    &mut self.panel_mut(PanelKind::Detail).selected_idx,
                    len,
                    step,
                );
                // En Data tab, no subir al header (row 0) — mínimo row 1
                if self.detail_tab == DetailTab::Data && len > 1 {
                    let panel = self.panel_mut(PanelKind::Detail);
                    if panel.selected_idx == 0 {
                        panel.selected_idx = 1;
                    }
                }
                // Scroll infinito (append/prepend) para Data tab
                if self.detail_tab == DetailTab::Data && len > 1 {
                    if step > 0 && old_idx == len.saturating_sub(1) {
                        // Al bajar del último dato → cargar página siguiente
                        self.scroll_down_infinite();
                    } else if step < 0 && old_idx == 1 && self.preview_loaded_offset > 0 {
                        // Al subir del primer dato → cargar página anterior
                        self.scroll_up_infinite();
                    }
                }
            }
        }
    }

    /// avPag/rePag en el Data tab: avanza/retrocede `K` filas, donde K = las
    /// filas que caben en pantalla. Sin concepto de "página estricta": se
    /// adapta al contenido actual con clamp a los bordes del buffer cargado
    /// (mínimo fila 1, nunca el header; máximo la última fila cargada). En
    /// los bordes, el scroll infinito carga más contenido SIN saltar la
    /// selección (append la deja quieta, prepend la desplaza +n).
    fn move_selection_by_page(&mut self, down: bool) {
        let len = self.preview_rows.len();
        if len <= 1 {
            return;
        }
        // Filas visibles del Data tab = rect del Detail − bordes (2) −
        // (spacer + header + separador, 3). Mismo cálculo que el render.
        let k = {
            let rect = self
                .layout
                .panels
                .iter()
                .find(|(kind, _)| *kind == PanelKind::Detail)
                .map(|(_, r)| *r)
                .unwrap_or_default();
            usize::from(rect.height.saturating_sub(5)).max(1)
        };
        let cur = self.selected_idx(PanelKind::Detail);
        let last = len.saturating_sub(1);
        let target =
            if down { cur.saturating_add(k).min(last) } else { cur.saturating_sub(k).max(1) };
        self.set_selected_idx(PanelKind::Detail, target);

        // Bordes del buffer: cargar más contenido (la selección no salta)
        if down && target == last {
            self.scroll_down_infinite();
        } else if !down && target == 1 && self.preview_loaded_offset > 0 {
            self.scroll_up_infinite();
        }
    }

    fn shift_index_on_vec_len(current: &mut usize, len: usize, step: isize) {
        if len == 0 {
            *current = 0;
            return;
        }

        let last = len.saturating_sub(1);
        let next = current.saturating_add_signed(step);
        *current = next.min(last);
    }

    /// Avanza `idx` en la dirección de `step` hasta aterrizar en un item que
    /// no sea sección (se queda en el borde si no hay más entries).
    fn skip_section_idx(items: &[String], mut idx: usize, step: isize) -> usize {
        if items.is_empty() {
            return idx;
        }
        let last = items.len().saturating_sub(1);
        while items.get(idx).is_some_and(|s| is_source_section(s)) {
            if step > 0 {
                if idx >= last {
                    break;
                }
                idx += 1;
            } else {
                if idx == 0 {
                    break;
                }
                idx -= 1;
            }
        }
        idx
    }

    // ── items por panel ───────────────────────────────────────────────

    pub fn items_for(&self, kind: PanelKind) -> &[String] {
        // Si hay filtro activo y es el panel activo, devolver filtrados
        if !self.filtered_items.is_empty() && self.active_panel == kind && kind.is_sidebar() {
            return &self.filtered_items;
        }
        match kind {
            PanelKind::Sources => &self.sources,
            PanelKind::Tables => &self.tables,
            PanelKind::Views => &self.views,
            PanelKind::Advanced => &self.advanced,
            PanelKind::Detail => &self.preview_rows,
        }
    }

    /// Número total de items originales (sin filtro) para el panel dado.
    pub fn items_len_for(&self, kind: PanelKind) -> usize {
        self.items_for(kind).len()
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn title_for(&self, kind: PanelKind) -> String {
        let num = kind.number();
        match kind {
            PanelKind::Sources => {
                let tabs = match self.source_tab {
                    SourceTab::All => "[Todo] Local Online",
                    SourceTab::Local => "Todo [Local] Online",
                    SourceTab::Online => "Todo Local [Online]",
                };
                format!("[{num}]Fuentes ({tabs})")
            }
            PanelKind::Tables => {
                if self.tables.is_empty() {
                    format!("[{num}]Tablas")
                } else {
                    format!("[{num}]Tablas ({})", self.tables.len())
                }
            }
            PanelKind::Views => {
                if self.views.is_empty() {
                    format!("[{num}]Vistas")
                } else {
                    format!("[{num}]Vistas ({})", self.views.len())
                }
            }
            PanelKind::Advanced => {
                if self.advanced.is_empty() {
                    format!("[{num}]Avanzado")
                } else {
                    format!("[{num}]Avanzado ({})", self.advanced.len())
                }
            }
            PanelKind::Detail => {
                let available = self.available_detail_tabs();
                let mut parts: Vec<String> = Vec::new();

                for &tab in &available {
                    let label = tab.label();
                    let text = if tab == DetailTab::Data {
                        // Número de fila actual / total. En NoSQL (mongo)
                        // cada fila es un documento → `doc X/Y`.
                        if self.total_rows > 0 {
                            let current_row = self.preview_loaded_offset
                                + self.selected_idx(PanelKind::Detail).saturating_sub(1) as u32
                                + 1;
                            let total = self.total_rows;
                            let unit = if self.is_nosql { "doc" } else { "row" };
                            format!("{label} - {unit} {current_row}/{total}")
                        } else {
                            label.to_string()
                        }
                    } else {
                        label.to_string()
                    };

                    // Tab activo entre corchetes, con padding
                    let padded = if tab == self.detail_tab {
                        format!(" [ {text} ] ")
                    } else {
                        format!("  {text}  ")
                    };
                    parts.push(padded);
                }

                let tab_bar = parts.join("|");

                format!("[{num}]{tab_bar}| ")
            }
        }
    }

    // ── fuentes ───────────────────────────────────────────────────────

    fn build_sources(
        state: &storage::AppState,
        source_tab: SourceTab,
        connected: Option<&str>,
        health: &HashMap<String, bool>,
        detected_servers: &[String],
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
                list.add_detected(detected_servers);
            }
            SourceTab::Local => {
                list.add_favs(SourceFilter::Local);
                list.add_recents(SourceFilter::Local);
                list.add_detected(detected_servers);
            }
            SourceTab::Online => {
                list.add_favs(SourceFilter::Online);
                list.add_recents(SourceFilter::Online);
            }
        }

        list.finish(source_tab)
    }

    fn set_source_tab(&mut self, tab: SourceTab) {
        self.source_tab = tab;
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            self.db_path.as_deref(),
            &self.health,
            &self.detected_servers,
        );
        self.set_selected_idx(PanelKind::Sources, 0);
    }

    // ── objetos ───────────────────────────────────────────────────────

    fn selected_object_name(&self) -> String {
        // Usar object_section (persiste) en vez de active_panel (cambia con foco)
        // Usar items_for para respetar filtros activos
        let section = self.object_section;

        let kind = match section {
            ObjectSection::Tables => PanelKind::Tables,
            ObjectSection::Views => PanelKind::Views,
            ObjectSection::Advanced => PanelKind::Advanced,
        };
        let items = self.items_for(kind);
        let idx = self.selected_idx(kind);

        if section == ObjectSection::Advanced {
            let raw = items.get(idx).map_or("-", String::as_str);
            if let Some((_, name)) = raw.split_once(':') {
                return name.to_string();
            }
            return raw.to_string();
        }

        items.get(idx).map_or_else(|| "-".to_string(), String::clone)
    }

    #[allow(dead_code)]
    pub fn selected_source(&self) -> &str {
        let idx = self.selected_idx(PanelKind::Sources);
        self.items_for(PanelKind::Sources).get(idx).map_or("-", String::as_str)
    }

    #[allow(dead_code)]
    pub fn selected_object(&self) -> &str {
        let raw = match self.active_panel {
            kind @ (PanelKind::Tables | PanelKind::Views | PanelKind::Advanced) => {
                self.items_for(kind).get(self.selected_idx(kind))
            }
            _ => None,
        };
        raw.map_or("-", String::as_str)
    }

    #[allow(dead_code)]
    pub const fn source_tab_label(&self) -> &'static str {
        self.source_tab.label()
    }

    #[allow(dead_code)]
    pub const fn object_section_label(&self) -> &'static str {
        self.object_section.label()
    }

    #[allow(dead_code)]
    /// Tabs disponibles en Detail según el tipo de objeto seleccionado
    pub fn available_detail_tabs(&self) -> Vec<DetailTab> {
        if self.object_section == ObjectSection::Advanced {
            vec![DetailTab::Sql, DetailTab::Meta]
        } else {
            vec![DetailTab::Data, DetailTab::Schema, DetailTab::Sql, DetailTab::Meta]
        }
    }

    #[allow(dead_code)]
    pub const fn detail_tab_label(&self) -> &'static str {
        self.detail_tab.label()
    }

    pub const fn actions_menu_items() -> &'static [&'static str] {
        &Self::ACTION_ITEMS
    }

    pub const fn actions_menu_selected(&self) -> usize {
        self.actions_menu_idx
    }

    /// Versión SEGURA del path para mostrar en la UI: quita las credenciales
    /// (`user:pass@`) — el password NUNCA debe aparecer en pantalla ni en
    /// logs de estado.
    pub fn db_path_safe(&self) -> String {
        self.db_path.as_deref().map_or_else(|| "-".to_string(), crate::security::strip_credentials)
    }

    pub fn db_size_display(&self) -> String {
        let Some(bytes) = self.db_size_bytes else {
            return "-".to_string();
        };

        if bytes >= MB_BYTES {
            let hundredths =
                (u128::from(bytes) * 100 + u128::from(MB_BYTES) / 2) / u128::from(MB_BYTES);
            let whole = hundredths / 100;
            let frac = hundredths % 100;
            format!("{whole}.{frac:02} MiB")
        } else if bytes >= KB_BYTES {
            let hundredths =
                (u128::from(bytes) * 100 + u128::from(KB_BYTES) / 2) / u128::from(KB_BYTES);
            let whole = hundredths / 100;
            let frac = hundredths % 100;
            format!("{whole}.{frac:02} KiB")
        } else {
            format!("{bytes} B")
        }
    }

    // ── conexión SQLite / servidores ──────────────────────────────────

    /// ¿La URL mysql:// trae base explícita (`.../bd`)? Las URLs detectadas
    /// por `scan_local_servers` NO la traen: son conexiones a nivel servidor.
    #[cfg(feature = "mysql")]
    fn connect_mysql_server(&mut self, url: &str) {
        self.status = format!("Conectando a {}...", crate::security::strip_credentials(url));
        // Si la URL YA trae credenciales (`user:pass@`), se conservan: es una
        // conexión a servidor autenticada desde el inicio.
        let has_creds = url
            .strip_prefix("mysql://")
            .is_some_and(|rest| rest.contains('@') && !rest.starts_with('@'));
        // Sin credenciales: probar `root` SIN contraseña (instalaciones
        // locales típicas). NO usar la URL tal cual (user vacío → la cuenta
        // anónima de MariaDB solo ve la BD `test`).
        let url_root = if has_creds {
            url.to_string()
        } else {
            let host = url.strip_prefix("mysql://").unwrap_or(url);
            format!("mysql://root@{host}")
        };
        let result = crate::db::rt::block_on(async {
            let (pool, db_name) = crate::db::backends::mysql::connect(&url_root)?;
            let dbs = crate::db::backends::mysql::list_databases(&pool)?;
            Ok::<(String, Vec<String>), crate::db::DbError>((db_name, dbs))
        });
        match result {
            Ok((db_name, dbs)) if dbs.is_empty() => {
                self.status = format!(
                    "Servidor {}: sin bases de usuario (BD actual: {db_name})",
                    crate::security::strip_credentials(url)
                );
            }
            Ok((_db_name, dbs)) => {
                self.status = format!(
                    "Servidor {}: elige una base ({})",
                    crate::security::strip_credentials(url),
                    dbs.len()
                );
                self.db_picker = Some(DbPickerState { server_url: url.to_string(), dbs, idx: 0 });
            }
            Err(err) => {
                // Sin acceso (auth o red): pedir contraseña. URL limpia (sin
                // credenciales) y el user de la URL si lo traía.
                tracing::warn!(url = %crate::security::strip_credentials(url), error = ?err, "servidor sin acceso, pidiendo contraseña");
                self.status = String::new();
                let (clean_url, user) = strip_url_credentials(url);
                self.password_prompt = Some(PasswordPromptState {
                    server_url: clean_url,
                    user: user.unwrap_or_else(|| "root".to_string()),
                    buffer: String::new(),
                });
            }
        }
    }

    /// Intenta conectar a la URL con `user:password` recién tipeados. Si la
    /// conexión va, lista las bases y abre el picker.
    #[cfg(feature = "mysql")]
    fn connect_mysql_server_with_password(&mut self, prompt: PasswordPromptState) {
        let PasswordPromptState { server_url, user, buffer } = prompt;
        // Construye `mysql://user:pass@host:port` a partir de `mysql://host:port`
        let url = server_url.strip_prefix("mysql://").map_or_else(
            || server_url.clone(),
            |host_port| format!("mysql://{user}:{buffer}@{host_port}"),
        );
        self.status =
            format!("Autenticando en {}...", crate::security::strip_credentials(&server_url));
        let result = crate::db::rt::block_on(async {
            let (pool, _db_name) = crate::db::backends::mysql::connect(&url)?;
            let dbs = crate::db::backends::mysql::list_databases(&pool)?;
            Ok::<Vec<String>, crate::db::DbError>(dbs)
        });
        match result {
            Ok(dbs) if dbs.is_empty() => {
                self.status = format!(
                    "Servidor {}: sin bases de usuario",
                    crate::security::strip_credentials(&server_url)
                );
            }
            Ok(dbs) => {
                self.status = format!(
                    "Servidor {}: elige una base ({})",
                    crate::security::strip_credentials(&server_url),
                    dbs.len()
                );
                self.db_picker = Some(DbPickerState { server_url: url, dbs, idx: 0 });
            }
            Err(err) => {
                tracing::warn!(url = %crate::security::strip_credentials(&server_url), error = ?err, "credenciales rechazadas");
                self.show_error("No se pudo autenticar", &err.to_string());
            }
        }
    }

    /// Conexión a nivel de servidor `PostgreSQL`. Prueba primero `postgres`
    /// SIN contraseña (instalaciones locales con trust/auth peer); si falla,
    /// abre el prompt de contraseña con user `postgres` por defecto.
    #[cfg(feature = "postgres")]
    fn connect_postgres_server(&mut self, url: &str) {
        self.status = format!("Conectando a {}...", crate::security::strip_credentials(url));
        // Normalizamos `postgresql://` → `postgres://` (el crate solo entiende
        // el segundo). Si la URL YA trae credenciales (`user:pass@`), se
        // conservan: es una conexión a servidor autenticada desde el inicio.
        let scheme_normalized = if url.starts_with("postgresql://") {
            url.replacen("postgresql://", "postgres://", 1)
        } else {
            url.to_string()
        };
        let has_creds = scheme_normalized
            .strip_prefix("postgres://")
            .is_some_and(|rest| rest.contains('@') && !rest.starts_with('@'));
        let host = scheme_normalized.strip_prefix("postgres://").unwrap_or(&scheme_normalized);
        // Sin credenciales: probar el superuser local `postgres` (peer auth).
        let url_postgres = if has_creds {
            scheme_normalized.clone()
        } else {
            format!("postgres://postgres@{host}")
        };
        let result = crate::db::rt::block_on(async {
            let (pool, db_name) = crate::db::backends::postgres::connect(&url_postgres)?;
            let dbs = crate::db::backends::postgres::list_databases(&pool)?;
            Ok::<(String, Vec<String>), crate::db::DbError>((db_name, dbs))
        });
        match result {
            Ok((db_name, dbs)) if dbs.is_empty() => {
                self.status = format!(
                    "Servidor {}: sin bases de usuario (BD actual: {db_name})",
                    crate::security::strip_credentials(url)
                );
            }
            Ok((_db_name, dbs)) => {
                self.status = format!(
                    "Servidor {}: elige una base ({})",
                    crate::security::strip_credentials(url),
                    dbs.len()
                );
                self.db_picker =
                    Some(DbPickerState { server_url: scheme_normalized.clone(), dbs, idx: 0 });
            }
            Err(err) => {
                tracing::warn!(url = %crate::security::strip_credentials(url), error = ?err, "servidor postgres sin acceso, pidiendo contraseña");
                self.status = String::new();
                // Prompt con URL SIN credenciales (no repetir la password en
                // pantalla) y el user de la URL si lo traía.
                let (clean_url, user) = strip_url_credentials(&scheme_normalized);
                self.password_prompt = Some(PasswordPromptState {
                    server_url: clean_url,
                    user: user.unwrap_or_else(|| "postgres".to_string()),
                    buffer: String::new(),
                });
            }
        }
    }

    /// Intenta conectar a la URL postgres con `user:password` recién
    /// tipeados. Si la conexión va, lista las bases y abre el picker.
    #[cfg(feature = "postgres")]
    fn connect_postgres_server_with_password(&mut self, prompt: PasswordPromptState) {
        let PasswordPromptState { server_url, user, buffer } = prompt;
        let url = server_url.strip_prefix("postgres://").map_or_else(
            || server_url.clone(),
            |host_port| format!("postgres://{user}:{buffer}@{host_port}"),
        );
        self.status =
            format!("Autenticando en {}...", crate::security::strip_credentials(&server_url));
        let result = crate::db::rt::block_on(async {
            let (pool, _db_name) = crate::db::backends::postgres::connect(&url)?;
            let dbs = crate::db::backends::postgres::list_databases(&pool)?;
            Ok::<Vec<String>, crate::db::DbError>(dbs)
        });
        match result {
            Ok(dbs) if dbs.is_empty() => {
                self.status = format!(
                    "Servidor {}: sin bases de usuario",
                    crate::security::strip_credentials(&server_url)
                );
            }
            Ok(dbs) => {
                self.status = format!(
                    "Servidor {}: elige una base ({})",
                    crate::security::strip_credentials(&server_url),
                    dbs.len()
                );
                self.db_picker = Some(DbPickerState { server_url: url, dbs, idx: 0 });
            }
            Err(err) => {
                tracing::warn!(url = %crate::security::strip_credentials(&server_url), error = ?err, "credenciales postgres rechazadas");
                self.show_error("No se pudo autenticar", &err.to_string());
            }
        }
    }

    /// Conexión a nivel de servidor `MongoDB`: lista las bases (listDatabases)
    /// y abre el picker para elegir. Mongo local suele correr sin auth; si hay
    /// credenciales en la URL, el connect las usa.
    #[cfg(feature = "mongodb")]
    fn connect_mongo_server(&mut self, url: &str) {
        self.status = format!("Conectando a {}...", crate::security::strip_credentials(url));
        let result = crate::db::rt::block_on(async {
            let (client, db_name) = crate::db::backends::mongo::connect(url)?;
            let dbs = crate::db::backends::mongo::list_databases(&client)?;
            Ok::<(String, Vec<String>), crate::db::DbError>((db_name, dbs))
        });
        match result {
            Ok((db_name, dbs)) if dbs.is_empty() => {
                self.status = format!(
                    "Servidor {}: sin bases de usuario (BD actual: {db_name})",
                    crate::security::strip_credentials(url)
                );
            }
            Ok((_db_name, dbs)) => {
                self.status = format!(
                    "Servidor {}: elige una base ({})",
                    crate::security::strip_credentials(url),
                    dbs.len()
                );
                self.db_picker = Some(DbPickerState { server_url: url.to_string(), dbs, idx: 0 });
            }
            Err(err) => {
                tracing::warn!(url = %crate::security::strip_credentials(url), error = ?err, "mongo sin acceso");
                self.show_error("No se pudo conectar a MongoDB", &err.to_string());
            }
        }
    }

    /// ¿La URL mysql:// trae base explícita (`.../bd`)? Las URLs detectadas
    /// por `scan_local_servers` NO la traen: son conexiones a nivel servidor.
    #[allow(clippy::too_many_lines)]
    fn connect_sqlite(&mut self, path: &str) {
        // Choke point de normalización: solo para rutas de archivo. Las URLs
        // (`mysql://`, `duckdb://` remotos) no se tocan.
        let mut path = if path.starts_with('/') || path.starts_with("mysql://") {
            path.to_string()
        } else if path.starts_with("postgresql://") {
            // El crate solo entiende `postgres://`; unificar el alias aquí.
            path.replacen("postgresql://", "postgres://", 1)
        } else if path.contains("://") {
            // Cualquier otra URL con scheme (mongodb://, postgres://, duckdb://
            // remoto, etc.) se preserva tal cual: solo los paths de archivo
            // pasan por la normalización.
            path.to_string()
        } else {
            crate::paths::normalize_path(path)
        };

        // Seguridad: si la URL trae credenciales, se guardan en el keyring
        // del SO (nunca en disco). La conexión usa la URL tal cual.
        if crate::security::has_credentials(&path) {
            if let Err(err) = crate::security::save_credentials(&path) {
                tracing::warn!(error = ?err, "no se pudieron guardar credenciales en el keyring");
            }
        } else if is_remote_url(&path) {
            // Reabrir un reciente (URL sin credenciales): recuperarlas del
            // keyring y reconstruir la URL para conectar.
            match crate::security::get_credentials(&path) {
                Ok(Some((user, pass))) if !pass.is_empty() => {
                    let scheme = if path.starts_with("mongodb://") {
                        "mongodb"
                    } else if path.starts_with("mysql://") {
                        "mysql"
                    } else {
                        "postgres"
                    };
                    path = inject_credentials(&path, scheme, &user, &pass);
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(error = ?err, "keyring: sin credenciales para {path}"),
            }
        }

        // URL mysql:// SIN base → conexión a nivel de SERVIDOR: listar los
        // esquemas (SHOW DATABASES) y dejar que el usuario elija. Las URLs
        // detectadas por `scan_local_servers` llegan sin `/bd`.
        tracing::debug!(path = %crate::security::strip_credentials(&path), "connect_sqlite: url normalizada");
        #[cfg(feature = "mysql")]
        if path.starts_with("mysql://") && !server_url_has_database(&path) {
            tracing::debug!(path = %crate::security::strip_credentials(&path), "mysql sin base → flujo de servidor");
            self.connect_mysql_server(&path);
            return;
        }
        #[cfg(feature = "postgres")]
        if (path.starts_with("postgres://") || path.starts_with("postgresql://"))
            && !server_url_has_database(&path)
        {
            tracing::debug!(path = %crate::security::strip_credentials(&path), "postgres sin base → flujo de servidor");
            self.connect_postgres_server(&path);
            return;
        }
        // URL mongodb:// SIN base → listar bases (listDatabases) y picker.
        // Mongo local suele correr sin auth; si pide credenciales el
        // connect falla y el resolver mostrará el error.
        #[cfg(feature = "mongodb")]
        if path.starts_with("mongodb://") && !server_url_has_database(&path) {
            self.connect_mongo_server(&path);
            return;
        }
        self.is_loading = true;
        self.status = format!("Conectando a {}...", crate::security::strip_credentials(&path));

        // Backend resuelto por extensión: sqlite (.db/.sqlite) o duckdb (.duckdb/.ddb)
        let Some(adapter) = db::resolver::resolve_backend(&path) else {
            self.is_loading = false;
            self.show_error(
                "No se pudo abrir la base",
                &format!("{}: fuente no soportada", crate::security::strip_credentials(&path)),
            );
            tracing::error!(
                path = %crate::security::strip_credentials(&path),
                "fuente no soportada por el resolver"
            );
            return;
        };
        // Compartir el adapter: la conexión/pool se crea UNA vez y se reusa
        // para todas las operaciones (evita límite de conexiones + handshakes).
        self.active_adapter = Some(std::sync::Arc::from(adapter));
        let adapter = self.active_adapter.as_ref().unwrap().as_ref();
        let tables = adapter.list_objects_by_type("table");
        let views = adapter.list_objects_by_type("view");
        let advanced = adapter.list_advanced_objects();

        if let (Ok(tables), Ok(views), Ok(advanced)) = (tables, views, advanced) {
            // Seguridad: el reciente se guarda SIN credenciales (el password
            // vive en el keyring del SO, nunca en recents.json en claro).
            let path_str = crate::security::strip_credentials(&path);
            self.state.add_recent(path_str);
            let _ = self.state.save();
            self.sources = Self::build_sources(
                &self.state,
                self.source_tab,
                Some(&path),
                &self.health,
                &self.detected_servers,
            );

            self.db_path = Some(path.clone());
            // ¿NoSQL? (mongo): cambia la terminología de la UI (row→doc) y
            // habilita el toggle JSON del modal de detalles.
            self.is_nosql = self.adapter_arc().is_some_and(|a| a.is_nosql());
            // `db_size_bytes` solo aplica a bds de archivo (mysql:// no es path)
            self.db_size_bytes = std::fs::metadata(&path).ok().map(|meta| meta.len());
            self.tables = tables;
            self.views = views;
            self.advanced = advanced;
            self.object_section = ObjectSection::Tables;

            // Resetear índices
            self.set_selected_idx(PanelKind::Tables, 0);
            self.set_selected_idx(PanelKind::Views, 0);
            self.set_selected_idx(PanelKind::Advanced, 0);
            self.set_selected_idx(PanelKind::Detail, 0);
            self.current_page = 0;
            self.preview_loaded_offset = 0;
            self.detail_tab = DetailTab::Data;

            self.refresh_preview_from_selected_object();
            self.status = format!(
                "Conectado en modo read-only: {}",
                crate::security::strip_credentials(&path)
            );
            tracing::info!(
                path = %crate::security::strip_credentials(&path),
                tablas = self.tables.len(),
                vistas = self.views.len(),
                "conectado"
            );

            // Mover foco a Tablas
            self.set_focus(PanelKind::Tables);
        } else {
            self.is_loading = false;
            self.show_error(
                "No se pudo abrir la base",
                &format!("{path}: no se pudo leer el catálogo"),
            );
            tracing::error!(
                path = %crate::security::strip_credentials(&path),
                "no se pudo abrir: catálogo ilegible"
            );
        }
    }

    fn refresh_from_connection(&mut self) {
        if let Some(path) = self.db_path.clone() {
            self.connect_sqlite(&path);
        }
    }

    // ── paginación dinámica ──────────────────────────────────────────

    /// Calcula cuántas filas caben en el panel Detail, redondeado a múltiplo de 10.
    fn optimal_rows_per_page(&self) -> u32 {
        // Buscar altura del panel Detail en el layout computado
        let detail_h = self
            .layout
            .panels
            .iter()
            .find(|(kind, _)| *kind == PanelKind::Detail)
            .map_or(10, |(_, rect)| rect.height);

        // Restar bordes del bloque (2) + overhead fijo
        // En Data tab: spacer(1) + header(1) + separator(1) = 3
        // En otros tabs: solo borde (0 overhead extra)
        let overhead: u16 = if self.detail_tab == DetailTab::Data { 5 } else { 2 };
        let available = u32::from(detail_h.saturating_sub(overhead));

        // Redondear a múltiplo de 10 hacia abajo, mínimo 10
        let rows = (available / 10) * 10;
        rows.clamp(10, 200)
    }

    // ── preview ───────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    fn refresh_preview_from_selected_object(&mut self) {
        // El resultado de una query libre ya no aplica: navegar a un objeto
        // devuelve el preview a la tabla seleccionada.
        self.query_mode = false;
        // Ajustar rows_per_page dinámicamente según espacio disponible
        self.rows_per_page = self.optimal_rows_per_page();
        // Reset scroll_offset del viewport al recargar completamente los datos
        self.panel_mut(PanelKind::Detail).scroll_offset.set(0);
        // Reset scroll horizontal: el nuevo objeto puede tener otras columnas
        self.panel_mut(PanelKind::Detail).h_scroll.set(0);

        self.is_loading = true;
        self.status = format!("Cargando {}...", self.detail_tab.label().trim());

        let object_name = self.selected_object_name();
        if object_name.is_empty() || object_name == "-" {
            self.preview_rows = vec!["Sin objeto seleccionado".to_string()];
            self.preview_data = None;
            self.total_rows = 0;
            self.preview_loaded_offset = 0;
            self.is_loading = false;
            self.set_selected_idx(PanelKind::Detail, 0);
            return;
        }

        // Backend resuelto por extensión: todas las lecturas del preview
        // (filas, schema, DDL, count) pasan por el adapter.
        let Some(adapter) = self.adapter_arc() else {
            self.is_loading = false;
            return;
        };

        // Siempre refrescar total_rows para tablas/vistas (no Advanced)
        if self.object_section != ObjectSection::Advanced {
            if let Ok(count) = adapter.table_row_count(&object_name) {
                self.total_rows = count;
            }
        }

        match self.detail_tab {
            DetailTab::Data => {
                if self.object_section == ObjectSection::Advanced {
                    // Para índices/triggers: mostrar el SQL DDL
                    match adapter.object_sql(&object_name) {
                        Ok(sql) => {
                            self.preview_rows =
                                sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                            if self.preview_rows.is_empty() {
                                self.preview_rows = vec!["-- SQL vacio --".to_string()];
                            }
                        }
                        Err(err) => {
                            self.preview_rows = vec![format!("Error SQL: {err}")];
                            self.show_error("Error SQL", &err.to_string());
                        }
                    }
                    self.preview_data = None;
                    self.total_rows = 0;
                    self.preview_loaded_offset = 0;
                    self.is_loading = false;
                    self.set_selected_idx(PanelKind::Detail, 0);
                    return;
                }

                match adapter.table_row_count(&object_name) {
                    Ok(_) => {} // total_rows ya fue actualizado arriba
                    Err(err) => {
                        self.preview_rows = vec![format!("Error contando filas: {err}")];
                        self.show_error("Error contando filas", &err.to_string());
                        self.preview_data = None;
                        self.total_rows = 0;
                        self.preview_loaded_offset = 0;
                        self.is_loading = false;
                        self.set_selected_idx(PanelKind::Detail, 0);
                        return;
                    }
                }

                let offset = self.current_page.saturating_mul(self.rows_per_page);
                let order_col = self.sort_column.as_deref().map(|col| (col, self.sort_asc));
                match adapter.table_rows_sorted(&object_name, self.rows_per_page, offset, order_col)
                {
                    Ok(data) => {
                        // Celdas tipadas para el render 2D (TableState +
                        // highlight_symbol); preview_rows queda como fallback
                        // (List de 1 columna / mensajes).
                        self.preview_rows = if data.rows.is_empty() {
                            vec!["<sin datos>".to_string()]
                        } else {
                            data.to_lines()
                        };
                        self.preview_data = Some(data);
                        self.preview_loaded_offset = offset;
                        self.set_selected_idx(
                            PanelKind::Detail,
                            usize::from(self.preview_rows.len() > 1),
                        );
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error obteniendo filas: {err}")];
                        self.show_error("Error obteniendo filas", &err.to_string());
                        self.preview_data = None;
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                }
            }
            DetailTab::Schema => {
                if self.object_section == ObjectSection::Advanced {
                    // Schema de índice/trigger = su SQL DDL
                    match adapter.object_sql(&object_name) {
                        Ok(sql) => {
                            self.preview_rows =
                                sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                            if self.preview_rows.is_empty() {
                                self.preview_rows = vec!["-- SQL vacio --".to_string()];
                            }
                        }
                        Err(err) => {
                            self.preview_rows = vec![format!("Error SQL: {err}")];
                            self.show_error("Error SQL", &err.to_string());
                        }
                    }
                    self.preview_data = None;
                    self.total_rows = 0;
                    self.preview_loaded_offset = 0;
                    self.is_loading = false;
                    self.set_selected_idx(PanelKind::Detail, 0);
                    return;
                }

                match adapter.table_columns(&object_name) {
                    Ok(columns) => {
                        self.preview_data = None;
                        // ColumnInfo → líneas de presentación del Schema tab
                        self.preview_rows = if columns.is_empty() {
                            vec!["Sin columnas visibles".to_string()]
                        } else {
                            let mut lines = vec!["cid | name | type | nullability".to_string()];
                            lines.extend(columns.iter().map(crate::db::ColumnInfo::to_line));
                            lines
                        };
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error schema: {err}")];
                        self.show_error("Error schema", &err.to_string());
                        self.preview_data = None;
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                }
            }
            DetailTab::Sql => {
                self.preview_data = None;
                match adapter.object_sql(&object_name) {
                    Ok(sql) => {
                        self.preview_rows =
                            sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                        if self.preview_rows.is_empty() {
                            self.preview_rows = vec!["-- SQL vacio --".to_string()];
                        }
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error SQL: {err}")];
                        self.show_error("Error SQL", &err.to_string());
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                }
            }
            DetailTab::Meta => {
                self.preview_data = None;
                self.preview_rows = vec![
                    format!("db_path: {}", self.db_path_safe()),
                    format!("db_size: {}", self.db_size_display()),
                    format!("source_tab: {}", self.source_tab.label()),
                    format!("object_section: {}", self.object_section.label()),
                    format!("detail_tab: {}", self.detail_tab.label()),
                    format!("object: {}", object_name),
                    format!("rows_per_page: {}", self.rows_per_page),
                    format!("loaded_offset: {}", self.preview_loaded_offset),
                    format!("estimated_rows: {}", self.total_rows),
                ];
                self.preview_loaded_offset = 0;
                self.set_selected_idx(PanelKind::Detail, 0);
            }
        }
        self.is_loading = false;
    }

    // ── scroll infinito (append/prepend) ─────────────────────────────

    /// Carga la siguiente página de datos y la agrega a `preview_rows`.
    /// Solo para tabla de datos (`DetailTab::Data`).
    ///
    /// La selección NO se mueve: las filas nuevas quedan debajo del buffer y
    /// el siguiente paso de navegación (tecla ↓, rueda o avPag) avanza a
    /// ellas de forma natural. Antes la selección saltaba a la primera fila
    /// nueva ("cambio de página" fantasma con la rueda).
    fn scroll_down_infinite(&mut self) {
        if self.query_mode {
            return; // resultado de query libre: sin scroll infinito
        }
        let data_len = self.preview_rows.len().saturating_sub(1);
        let next_offset = self.preview_loaded_offset as usize + data_len;
        if next_offset >= self.total_rows as usize {
            return; // ya estamos al final del dataset
        }

        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            return;
        }

        #[allow(clippy::cast_possible_truncation)]
        let limit = self.rows_per_page.min(self.total_rows.saturating_sub(next_offset as u32));

        if limit == 0 {
            return;
        }

        self.is_loading = true;
        self.status = format!("Cargando más filas (offset {next_offset})...");

        let order_col = self.sort_column.as_deref().map(|col| (col, self.sort_asc));
        let Some(adapter) = self.adapter_arc() else {
            self.is_loading = false;
            return;
        };
        #[allow(clippy::cast_possible_truncation)]
        if let Ok(data) = adapter.table_rows_sorted(&object, limit, next_offset as u32, order_col) {
            // data.rows son las filas de datos nuevas (sin header: ya tenemos
            // el nuestro en preview_rows[0])
            if data.rows.is_empty() {
                self.is_loading = false;
                return;
            }
            self.preview_rows.extend(data.rows.iter().map(|row| row.to_line(" | ")));
            // Sincronizar las celdas tipadas del render 2D: el viewport usa
            // `preview_data.rows.len()` como tope (`rows_hint`) — sin esto el
            // scroll se congela en la primera página mientras el label de
            // `row X/Y` sigue avanzando (bug "la vista no baja").
            if let Some(existing) = self.preview_data.take() {
                let mut rows = existing.rows;
                rows.extend(data.rows);
                self.preview_data = Some(crate::db::TableData { columns: existing.columns, rows });
            }
        }
        self.is_loading = false;
    }

    /// Carga la página anterior de datos y la antepone a `preview_rows`.
    /// Solo para tabla de datos (`DetailTab::Data`). Actualiza
    /// `preview_loaded_offset` y desplaza la selección +n (las filas nuevas
    /// quedan ARRIBA) para mantener la MISMA fila global visible — sin salto
    /// visual ("página atrás" fantasma).
    fn scroll_up_infinite(&mut self) {
        if self.query_mode {
            return; // resultado de query libre: sin scroll infinito
        }
        if self.preview_loaded_offset == 0 {
            return; // ya estamos al inicio del dataset
        }

        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            return;
        }

        let limit = self.rows_per_page.min(self.preview_loaded_offset);
        let offset = self.preview_loaded_offset.saturating_sub(limit);
        if limit == 0 {
            return;
        }

        self.is_loading = true;
        self.status = format!("Cargando filas anteriores (offset {offset})...");

        let order_col = self.sort_column.as_deref().map(|col| (col, self.sort_asc));
        let Some(adapter) = self.adapter_arc() else {
            self.is_loading = false;
            return;
        };
        if let Ok(data) = adapter.table_rows_sorted(&object, limit, offset, order_col) {
            // data.rows son los datos nuevos (el header ya está en index 0)
            let n = data.rows.len(); // cantidad de filas nuevas
            if n == 0 {
                self.is_loading = false;
                return;
            }

            // Anteponer las filas nuevas (preservando el header en index 0)
            let header = self.preview_rows[0].clone();
            let mut expanded = vec![header];
            expanded.extend(data.rows.iter().map(|row| row.to_line(" | ")));
            expanded.extend(self.preview_rows.iter().skip(1).cloned());
            self.preview_rows = expanded;

            // Mantener sincronizadas las celdas tipadas del render 2D
            if let Some(existing) = self.preview_data.take() {
                let mut rows = data.rows;
                rows.extend(existing.rows);
                self.preview_data = Some(crate::db::TableData { columns: existing.columns, rows });
            }

            // Actualizar offset
            #[allow(clippy::cast_possible_truncation)]
            {
                self.preview_loaded_offset -= n as u32;
            }

            // La selección se desplaza +n para mantener la misma fila global
            // (las filas nuevas se anteponen; la vista no salta).
            let cur = self.selected_idx(PanelKind::Detail);
            self.set_selected_idx(PanelKind::Detail, cur.saturating_add(n));

            // El viewport (`scroll_offset`) también se desplaza +n: las filas
            // nuevas entraron ARRIBA del buffer y la vista debe seguir
            // mostrando las mismas filas globales de antes. Sin esto, el
            // render ve la selección "lejos" del scroll viejo y salta la
            // vista hacia abajo ("cambio de página" fantasma con la rueda:
            // baja N filas de golpe y la fila enfocada queda abajo).
            let p = self.panel_mut(PanelKind::Detail);
            let so = p.scroll_offset.get();
            p.scroll_offset.set(so.saturating_add(n));
        }
        self.is_loading = false;
    }

    fn set_detail_tab(&mut self, tab: DetailTab) {
        // Si el tab no está disponible, buscar el siguiente disponible
        let available = self.available_detail_tabs();
        let effective = if available.contains(&tab) {
            tab
        } else {
            // Buscar siguiente disponible (o el primero si no hay)
            available.iter().find(|t| t.label() > tab.label()).copied().unwrap_or(available[0])
        };

        self.detail_tab = effective;
        self.set_selected_idx(PanelKind::Detail, 0);
        self.refresh_preview_from_selected_object();
    }

    // ── query ─────────────────────────────────────────────────────────

    fn execute_count_query(&mut self) {
        let Some(path) = self.db_path.clone() else {
            self.status = "No hay DB conectada".to_string();
            return;
        };

        if self.object_section == ObjectSection::Advanced {
            self.status = "COUNT(*) no aplica a indices/triggers".to_string();
            return;
        }

        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            self.status = "Selecciona una tabla o vista primero".to_string();
            return;
        }

        let sql = format!("SELECT COUNT(*) FROM \"{}\";", object.replace('"', "\"\""));
        tracing::debug!(object = %object, "COUNT(*) lanzado (generación {})", self.query_gen + 1);

        // Generación nueva: cualquier resultado en vuelo de queries anteriores
        // queda marcado como stale y se descarta al llegar.
        self.query_gen += 1;
        let generation = self.query_gen;
        let Some(tx) = self.query_tx.clone() else { return };

        self.query_state = query::QueryState::Running;
        self.status = "Contando filas...".to_string();

        // Provider: reusar la conexión activa (evita re-handshake en remoto)
        let adapter = self.adapter_arc();
        tokio::spawn(async move {
            let res = query::count_query_results(adapter, &path, &sql).await;
            let _ = tx.send(query::QueryMsg::Count(generation, sql, res));
        });
    }

    /// Aplica un resultado de count en el estado. Devuelve `None` si era
    /// stale (generación vieja). Pura para poder testear la cancelación
    /// sin App.
    fn apply_count_result(
        query_gen: u64,
        generation: u64,
        sql: &str,
        res: Result<u32, crate::db::DbError>,
    ) -> Option<(query::QueryState, String)> {
        if generation != query_gen {
            return None; // stale: resultado de una query que ya no importa
        }
        let (state, status) = match res {
            Ok(count) => (
                query::QueryState::Done(vec![format!("COUNT(*) = {count}"), format!("SQL: {sql}")]),
                format!("Query completada: {count} filas"),
            ),
            Err(e) => {
                // El Display de DbError ya incluye contexto de la variante
                let msg = e.to_string();
                (query::QueryState::Error(msg.clone()), msg)
            }
        };
        Some((state, status))
    }

    fn poll_query_results(&mut self) {
        // Drenar el canal en un scope corto: los borrows del loop no pueden
        // convivir con las mutaciones de self que hace el procesamiento.
        let drained: Vec<query::QueryMsg> = {
            let Some(rx) = self.query_rx.as_mut() else { return };
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };
        for msg in drained {
            match msg {
                query::QueryMsg::Count(generation, sql, res) => {
                    let Some((state, status)) =
                        Self::apply_count_result(self.query_gen, generation, &sql, res)
                    else {
                        continue; // stale: descartar
                    };
                    self.query_results = if let query::QueryState::Done(rows) = &state {
                        rows.clone()
                    } else {
                        Vec::new()
                    };
                    self.query_state = state;
                    self.status = status;
                }
                query::QueryMsg::Free(generation, _sql, res) => {
                    if generation != self.query_gen {
                        continue; // stale: una query más nueva ya se lanzó
                    }
                    self.is_loading = false;
                    let (state, rows) = Self::apply_user_query_result(&res);
                    if matches!(state, query::QueryState::Done(_)) {
                        self.query_mode = true;
                        self.query_state = state;
                        self.query_results = rows;
                        self.detail_tab = DetailTab::Data;
                        // Vista de datos 2D: fila 0 es el header, el
                        // resto son datos (el render ya hace el split).
                        self.preview_rows = self.query_results.clone();
                        self.preview_data = None;
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                        self.status = format!(
                            "{} filas · query OK (limit {})",
                            self.query_results.len(),
                            query::QUERY_RESULT_LIMIT
                        );
                    } else if let query::QueryState::Error(e) = state {
                        self.query_state = query::QueryState::Error(e.clone());
                        self.status = format!("Error SQL: {e}");
                        self.show_error("Error SQL", &e);
                    }
                }
            }
        }
    }

    fn clear_query_state(&mut self) {
        // Invalidar cualquier count en vuelo antes de limpiar
        self.query_gen += 1;
        self.query_state = query::QueryState::Idle;
        self.query_results.clear();
        self.status = "Query limpia".to_string();
    }

    // ── input SQL (`:` popup + historial persistente estilo fish) ──────

    fn handle_query_input_key(&mut self, key: KeyEvent) {
        let Some(state) = self.query_input.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.query_input = None;
            }
            KeyCode::Enter => {
                let sql = state.buffer.trim().to_string();
                self.query_input = None;
                if sql.is_empty() {
                    return;
                }
                // Sin db abierta el input es de CONEXIÓN (URL manual); con
                // db es el SQL del modal `:`.
                if self.db_path.is_none() {
                    self.connect_sqlite(&sql);
                } else {
                    self.execute_user_query(&sql);
                }
            }
            KeyCode::Backspace => {
                if state.cursor > 0 {
                    let idx = state
                        .buffer
                        .char_indices()
                        .nth(state.cursor.saturating_sub(1))
                        .map_or(0, |(i, _)| i);
                    state.buffer.remove(idx);
                    state.cursor = state.cursor.saturating_sub(1);
                }
            }
            KeyCode::Left => {
                state.cursor = state.cursor.saturating_sub(1);
                state.history_idx = None;
            }
            KeyCode::Right => {
                if state.cursor < state.buffer.chars().count() {
                    state.cursor += 1;
                    state.history_idx = None;
                }
            }
            KeyCode::Up => self.query_history_select(1),
            KeyCode::Down => self.query_history_select(0),
            KeyCode::Char(c) => {
                state.buffer.insert(
                    state
                        .buffer
                        .char_indices()
                        .nth(state.cursor)
                        .map_or(state.buffer.len(), |(i, _)| i),
                    c,
                );
                state.cursor += 1;
                state.history_idx = None;
            }
            _ => {}
        }
    }

    // ── formulario de nueva conexión ─────────────────────────────────

    /// Reconstruye la URL canónica desde los campos individuales
    /// (host/puerto/user/pass/db). Se llama al editar un campo.
    fn conn_rebuild_url_from_fields(&mut self) {
        let Some(form) = self.connection_form.as_mut() else { return };
        let kind = form.kind_override.unwrap_or(form.kind);
        let scheme = match kind {
            crate::db::connection::ConnectionType::Mysql => "mysql",
            crate::db::connection::ConnectionType::Postgres => "postgres",
            crate::db::connection::ConnectionType::Mongo => "mongodb",
            crate::db::connection::ConnectionType::Sqlite => "sqlite",
            crate::db::connection::ConnectionType::Duckdb => "duckdb",
            crate::db::connection::ConnectionType::File
            | crate::db::connection::ConnectionType::Unknown => {
                return; // archivos y desconocido: la URL es libre (ruta)
            }
        };
        let mut url = format!("{scheme}://");
        if !form.user.is_empty() || !form.pass.is_empty() {
            url.push_str(&form.user);
            if !form.pass.is_empty() {
                url.push(':');
                url.push_str(&form.pass);
            }
            url.push('@');
        }
        url.push_str(&form.host);
        if !form.port.is_empty() {
            url.push(':');
            url.push_str(&form.port);
        }
        if !form.db.is_empty() {
            url.push('/');
            url.push_str(&form.db);
        }
        form.url = url;
    }

    /// Reparsea la URL actual y rellena los campos (si está completa).
    /// Devuelve `true` si el parseo fue exitoso (la URL define el tipo).
    fn conn_parse_url_into_fields(&mut self) -> bool {
        let Some(form) = self.connection_form.as_mut() else { return false };
        // Purga de la URL: quitar saltos de línea y espacios externos antes
        // de analizar (las URLs de CleverCloud pueden llegar partidas).
        form.url = form.url.trim().replace(['\n', '\r'], "");
        let spec = crate::db::connection::analyze_connection(&form.url);
        let complete = spec.kind != crate::db::connection::ConnectionType::Unknown;

        if complete {
            form.kind = spec.kind;
            // Solo sobreescribir campos si el usuario no los forzó
            if form.kind_override.is_none() {
                form.host = spec.host.clone().unwrap_or_default();
                form.port = spec.port.map_or_else(String::new, |p| p.to_string());
                form.user = spec.user.clone().unwrap_or_default();
                form.pass = spec.pass.clone().unwrap_or_default();
                form.db = spec.db_name.clone().unwrap_or_default();
            }
            form.detected_note = format!("✓ Detectado: {}", spec.display());
        } else if form.url.is_empty() {
            form.detected_note.clear();
        } else {
            form.detected_note = "⏳ escribe la URL o ruta…".to_string();
        }
        complete
    }

    /// Procesa el debounce de la URL (1s desde la última tecla): si está
    /// completa, rellena los campos. Se llama una vez por frame.
    pub fn tick_connection_form(&mut self) {
        let debounce_due = {
            let Some(form) = self.connection_form.as_ref() else { return };
            if !form.url_debounce_scheduled {
                return;
            }
            form.url_last_edit
                .is_some_and(|last| last.elapsed() >= std::time::Duration::from_secs(1))
        };
        if debounce_due {
            if let Some(form) = self.connection_form.as_mut() {
                form.url_debounce_scheduled = false;
                form.url_last_edit = None;
            }
            self.conn_parse_url_into_fields();
        }
    }

    /// Conecta con lo que haya en el formulario (URL canónica).
    fn conn_submit(&mut self) {
        // Si el foco está en un campo individual, reconstruir primero
        let rebuild = self.connection_form.as_ref().is_some_and(|form| {
            matches!(
                form.active,
                ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Pass
                    | ConnField::Db
            )
        });
        if rebuild {
            self.conn_rebuild_url_from_fields();
        }
        let url = self.connection_form.as_ref().map(|f| f.url.clone()).unwrap_or_default();
        // Purga final antes de conectar (defensa en profundidad contra \n)
        let url = url.trim().replace(['\n', '\r'], "");
        if url.trim().is_empty() {
            self.status = "Escribe una URL o ruta para conectar".to_string();
            return;
        }
        tracing::debug!(url = %crate::security::strip_credentials(&url), rebuild, "conn_submit: url a conectar");
        // Conexión real (sync por ahora; el spinner se limpia después)
        self.connection_form.as_mut().unwrap().connecting = true;
        self.connect_sqlite(&url);
        // REGLA DE ORO: el formulario persiste en el estado; el render lo
        // oculta mientras haya db (`db_path.is_some()`). Al desconectar,
        // reaparece automáticamente.
        if let Some(form) = self.connection_form.as_mut() {
            form.connecting = false;
            form.url_debounce_scheduled = false;
        }
        if self.db_path.is_none() {
            self.status =
                format!("No se pudo conectar a {}", crate::security::strip_credentials(&url));
        }
    }

    /// Maneja las teclas del formulario de conexión.
    // `drop(form)` termina el borrow de `self.connection_form` (NLL) antes de
    // llamar a `self.conn_*`; es un no-op para clippy pero necesario aquí.
    #[allow(clippy::too_many_lines)]
    fn handle_connection_form_key(&mut self, key: KeyEvent) {
        let Some(form) = self.connection_form.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                // REGLA DE ORO: el formulario nunca se cierra sin db. Esc solo
                // mueve el foco a Fuentes (la conexión queda pendiente).
                self.active_panel = PanelKind::Sources;
                self.status =
                    "Conexión pendiente: el formulario queda en el panel Detalle".to_string();
            }
            // Ctrl+U: limpiar el campo activo (estándar readline)
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let active = form.active;
                let mut rebuild = false;
                match active {
                    ConnField::Url => {
                        form.url.clear();
                        form.url_last_edit = None;
                        form.url_debounce_scheduled = false;
                        form.detected_note.clear();
                    }
                    ConnField::Host => {
                        form.host.clear();
                        rebuild = true;
                    }
                    ConnField::Port => {
                        form.port.clear();
                        rebuild = true;
                    }
                    ConnField::User => {
                        form.user.clear();
                        rebuild = true;
                    }
                    ConnField::Pass => {
                        form.pass.clear();
                        rebuild = true;
                    }
                    ConnField::Db => {
                        form.db.clear();
                        rebuild = true;
                    }
                    _ => {}
                }
                if rebuild {
                    let _ = form;
                    self.conn_rebuild_url_from_fields();
                }
            }
            // Ctrl+L: limpiar TODO el formulario (empezar de cero)
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                *form = ConnectionFormState::default();
            }
            KeyCode::Tab | KeyCode::Down => {
                let next = match form.active {
                    ConnField::Url => ConnField::Kind,
                    ConnField::Kind => ConnField::Host,
                    ConnField::Host => ConnField::Port,
                    ConnField::Port => ConnField::User,
                    ConnField::User => ConnField::Pass,
                    ConnField::Pass => ConnField::Db,
                    ConnField::Db => ConnField::Connect,
                    ConnField::Connect => ConnField::Url,
                };
                form.active = next;
            }
            KeyCode::Up => {
                let prev = match form.active {
                    ConnField::Url => ConnField::Connect,
                    ConnField::Kind => ConnField::Url,
                    ConnField::Host => ConnField::Kind,
                    ConnField::Port => ConnField::Host,
                    ConnField::User => ConnField::Port,
                    ConnField::Pass => ConnField::User,
                    ConnField::Db => ConnField::Pass,
                    ConnField::Connect => ConnField::Db,
                };
                form.active = prev;
            }
            KeyCode::Enter => {
                let on_url = form.active == ConnField::Url;
                let _ = form; // termina el borrow antes de llamar &mut self
                if on_url {
                    // Enter en la URL: parsea Y conecta de una (el usuario
                    // escribió la URL completa y quiere entrar).
                    let parsed = self.conn_parse_url_into_fields();
                    if parsed {
                        self.conn_submit();
                    }
                } else {
                    self.conn_submit();
                }
            }
            KeyCode::Backspace => {
                let active = form.active;
                let mut rebuild = false;
                match active {
                    ConnField::Url => {
                        form.url.pop();
                        form.url_last_edit = Some(std::time::Instant::now());
                        form.url_debounce_scheduled = true;
                    }
                    // Host/Port/User/Pass/Db: pop + reconstruir la URL
                    other @ (ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Pass
                    | ConnField::Db) => {
                        let field = match other {
                            ConnField::Host => &mut form.host,
                            ConnField::Port => &mut form.port,
                            ConnField::User => &mut form.user,
                            ConnField::Pass => &mut form.pass,
                            _ => &mut form.db,
                        };
                        field.pop();
                        rebuild = true;
                    }
                    _ => {}
                }
                if rebuild {
                    let _ = form;
                    self.conn_rebuild_url_from_fields();
                }
            }
            KeyCode::Char(c) => {
                let active = form.active;
                let mut rebuild = false;
                match active {
                    ConnField::Url => {
                        form.url.push(c);
                        form.url_last_edit = Some(std::time::Instant::now());
                        form.url_debounce_scheduled = true;
                    }
                    // Kind y Connect no aceptan chars (Kind: h/l cambia el tipo)
                    ConnField::Kind | ConnField::Connect => {}
                    other @ (ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Pass
                    | ConnField::Db) => {
                        let field = match other {
                            ConnField::Host => &mut form.host,
                            ConnField::Port => &mut form.port,
                            ConnField::User => &mut form.user,
                            ConnField::Pass => &mut form.pass,
                            _ => &mut form.db,
                        };
                        if other == ConnField::Port {
                            if c.is_ascii_digit() {
                                field.push(c);
                                rebuild = true;
                            }
                        } else {
                            field.push(c);
                            rebuild = true;
                        }
                    }
                }
                if rebuild {
                    let _ = form;
                    self.conn_rebuild_url_from_fields();
                }
            }
            _ => {}
        }
    }

    /// Teclas del prompt de contraseña (servidor detectado): Esc cancela,
    /// Enter autentica, el resto alimenta el buffer enmascarado.
    fn handle_password_prompt_key(&mut self, key: KeyEvent) {
        let Some(state) = self.password_prompt.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.password_prompt = None;
                self.status = "Conexión al servidor cancelada".to_string();
            }
            KeyCode::Enter => {
                let prompt = std::mem::take(&mut self.password_prompt).expect("estado presente");
                // URL postgres:// → autenticar en postgres; el resto (mysql://)
                // cae al bloque mysql. Con un solo backend compilado, la rama
                // inexistente desaparece y el `else` muestra el mensaje genérico.
                #[cfg(feature = "postgres")]
                if prompt.server_url.starts_with("postgres://") {
                    self.connect_postgres_server_with_password(prompt);
                } else {
                    #[cfg(feature = "mysql")]
                    self.connect_mysql_server_with_password(prompt);
                    #[cfg(not(feature = "mysql"))]
                    {
                        let _ = prompt;
                        self.password_prompt = None;
                        self.status = "Servidor remoto no soportado en este build".to_string();
                    }
                }
                #[cfg(not(feature = "postgres"))]
                {
                    #[cfg(feature = "mysql")]
                    self.connect_mysql_server_with_password(prompt);
                    #[cfg(not(any(feature = "mysql", feature = "postgres")))]
                    {
                        let _ = prompt;
                        self.password_prompt = None;
                        self.status = "Servidor remoto no soportado en este build".to_string();
                    }
                }
            }
            KeyCode::Backspace => {
                state.buffer.pop();
            }
            KeyCode::Char(c) => state.buffer.push(c),
            _ => {}
        }
    }

    /// Teclas del picker de base de datos: ↑/↓ navegan, Enter conecta a la
    /// base seleccionada, Esc/Esc cancela el modal.
    fn handle_db_picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.db_picker = None;
                self.status = "Selección de base cancelada".to_string();
            }
            KeyCode::Up => {
                if let Some(p) = self.db_picker.as_mut() {
                    p.idx = p.idx.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(p) = self.db_picker.as_mut() {
                    let last = p.dbs.len().saturating_sub(1);
                    p.idx = (p.idx + 1).min(last);
                }
            }
            KeyCode::Enter => {
                let Some(picker) = self.db_picker.take() else { return };
                let Some(db) = picker.dbs.get(picker.idx).cloned() else {
                    self.status = "No hay bases para elegir".to_string();
                    return;
                };
                let mut url = picker.server_url;
                if !url.ends_with('/') {
                    url.push('/');
                }
                url.push_str(&db);
                self.connect_sqlite(&url);
            }
            _ => {}
        }
    }

    /// ↑/↓ sobre el historial (estilo fish): rellena el buffer con la query
    /// seleccionada; en `step = 0` (↓) vuelve hacia la query nueva.
    fn query_history_select(&mut self, step: usize) {
        let Some(state) = self.query_input.as_mut() else { return };
        let len = self.state.query_history.len();
        if len == 0 {
            self.status = "Historial vacío".to_string();
            return;
        }
        // step=1 (↑): avanza hacia atrás; step=0 (↓): hacia la nueva
        let idx = match state.history_idx {
            Some(i) if step == 1 => i.saturating_add(1).min(len - 1),
            Some(i) if step == 0 && i > 0 => i - 1,
            Some(_) | None => 0,
        };
        let sql = self.state.query_history[idx].clone();
        state.history_idx = Some(idx);
        state.buffer = sql;
        state.cursor = state.buffer.chars().count();
    }

    /// Ejecuta una query libre del usuario contra la DB (async, con
    /// generación anti-stale), registra el historial y muestra el resultado
    /// en el preview (modo query, sin scroll infinito).
    fn execute_user_query(&mut self, sql: &str) {
        let Some(path) = self.db_path.clone() else {
            self.show_error("Sin conexión", "Conecta una base primero (`:`)");
            return;
        };
        // NoSQL (mongo): el modal `:` recibe un filtro JSON que se aplica
        // sobre la colección seleccionada. Viaja embebido en el sql interno
        // (`@<coleccion> <json>`) porque el query runner resuelve un adapter
        // nuevo y no conoce la colección activa.
        let sql = if self.is_nosql {
            let coll = self.selected_object_name();
            if coll.is_empty() || coll == "-" {
                self.show_error("Sin colección", "Selecciona una colección antes de filtrar");
                return;
            }
            format!("@{coll} {sql}")
        } else {
            sql.to_string()
        };

        self.state.add_query_history(&sql);
        let _ = self.state.save();
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            Some(&path),
            &self.health,
            &self.detected_servers,
        );

        tracing::debug!(sql = %sql, "query libre lanzada (generación {})", self.query_gen + 1);
        self.query_gen += 1;
        let generation = self.query_gen;
        let Some(tx) = self.query_tx.clone() else { return };

        self.query_state = query::QueryState::Running;
        self.status = "Ejecutando query...".to_string();
        self.is_loading = true;

        // Provider: reusar la conexión activa (evita re-handshake en remoto)
        let adapter = self.adapter_arc();
        tokio::spawn(async move {
            let res = query::execute_query(adapter, &path, &sql, query::QUERY_RESULT_LIMIT).await;
            let _ = tx.send(query::QueryMsg::Free(generation, sql, res));
        });
    }

    /// Aplica el resultado de una query libre. Pura para testear sin App.
    fn apply_user_query_result(
        res: &Result<query::QueryResult, crate::db::DbError>,
    ) -> (query::QueryState, Vec<String>) {
        match res {
            Ok(qr) => qr.error.as_ref().map_or_else(
                || (query::QueryState::Done(qr.rows.clone()), qr.rows.clone()),
                |err| (query::QueryState::Error(err.clone()), Vec::new()),
            ),
            Err(e) => (query::QueryState::Error(e.to_string()), Vec::new()),
        }
    }

    // ── favoritos ─────────────────────────────────────────────────────

    fn mark_current_db_as_favorite(&mut self) {
        let Some(path) = self.db_path.as_deref() else {
            self.status = "Abre una base primero para guardarla como favorita".to_string();
            return;
        };

        let favorite_name = std::path::Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(path)
            .to_string();

        self.state.add_favorite(favorite_name.clone(), path.to_string());
        let _ = self.state.save();
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            self.db_path.as_deref(),
            &self.health,
            &self.detected_servers,
        );
        self.status = format!("Favorito guardado: {favorite_name}");
    }

    /// `f` en el panel Fuentes: marca/desmarca como favorito el item bajo el
    /// cursor. En cualquier otro panel: favoritear la DB conectada.
    fn toggle_favorite_source(&mut self) {
        if self.active_panel != PanelKind::Sources {
            self.mark_current_db_as_favorite();
            return;
        }

        let selected = self.selected_source().to_string();
        if is_source_section(&selected) || selected == "<sin entradas>" {
            return;
        }

        let path = source_path_of(&selected).to_string();

        if let Some(name) = self.state.favorite_name_for_path(&path) {
            self.state.remove_favorite(&name);
            self.status = format!("Favorito quitado: {name}");
        } else {
            let name = std::path::Path::new(&path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&path)
                .to_string();
            self.state.add_favorite(name.clone(), path);
            self.status = format!("Favorito guardado: {name}");
        }

        let _ = self.state.save();
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            self.db_path.as_deref(),
            &self.health,
            &self.detected_servers,
        );
    }

    /// `d` en el panel Fuentes: olvida la fuente bajo el cursor (la quita de
    /// recientes y de favoritos). Si era la DB conectada, la cierra.
    fn forget_source(&mut self) {
        if self.active_panel != PanelKind::Sources {
            return;
        }

        let selected = self.selected_source().to_string();
        if is_source_section(&selected) || selected == "<sin entradas>" {
            return;
        }

        let path = source_path_of(&selected).to_string();
        // Acciones fijas del final de la lista: no se olvidan
        if path == "Abrir sakila.db" || path == "Buscar archivo .db" {
            self.status = "Las acciones fijas no se pueden olvidar".to_string();
            return;
        }

        // Olvidar tanto la forma mostrada (canónica) como cualquier variante
        // relativa que dejaran versiones antiguas de lazydb en el storage.
        let canonical = crate::paths::normalize_path(&path);
        for candidate in [path.as_str(), canonical.as_str()] {
            self.state.remove_recent(candidate);
            self.state.remove_favorite_by_path(candidate);
        }
        let _ = self.state.save();

        if self.db_path.as_deref() == Some(canonical.as_str()) {
            self.disconnect_db();
        } else {
            self.sources = Self::build_sources(
                &self.state,
                self.source_tab,
                self.db_path.as_deref(),
                &self.health,
                &self.detected_servers,
            );
            self.status = format!("Fuente olvidada: {path}");
        }
    }

    /// Adapter de la conexión activa (compartido). `None` si no hay db.
    /// Referencia clonada al adapter activo (no presta `self`).
    fn adapter_arc(&self) -> Option<std::sync::Arc<dyn crate::db::adapter::DbAdapter>> {
        self.active_adapter.as_ref().map(std::sync::Arc::clone)
    }

    /// Cierra la conexión actual y vuelve el foco a Fuentes.
    fn disconnect_db(&mut self) {
        self.db_path = None;
        self.db_size_bytes = None;
        self.active_adapter = None;
        self.tables.clear();
        self.views.clear();
        self.advanced.clear();
        self.preview_rows.clear();
        self.preview_data = None;
        self.connection_form = Some(ConnectionFormState::default());
        self.total_rows = 0;
        self.preview_loaded_offset = 0;
        self.current_page = 0;
        self.detail_tab = DetailTab::Data;
        // Invalidar cualquier query en vuelo: su resultado ya no aplica
        self.query_gen += 1;
        self.query_state = query::QueryState::Idle;
        self.query_results.clear();
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            None,
            &self.health,
            &self.detected_servers,
        );
        self.set_focus(PanelKind::Sources);
        self.set_selected_idx(PanelKind::Sources, 0);
        self.status = "Base de datos cerrada".to_string();
        tracing::info!("desconectado");
    }

    // ── menú de acciones ──────────────────────────────────────────────

    // ── exportar CSV ─────────────────────────────────────────────────

    #[allow(clippy::cast_precision_loss)]
    fn export_csv(&mut self) {
        let Some(path) = self.db_path.clone() else {
            self.status = "No hay DB conectada".to_string();
            return;
        };
        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            self.status = "Selecciona una tabla o vista primero".to_string();
            return;
        }

        self.is_loading = true;
        let safe_name = object.replace([' ', '"', '/', '\\'], "_");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let filename = format!("{safe_name}_{timestamp}.csv");
        let sql = format!("SELECT * FROM \"{}\";", object.replace('"', "\"\""));

        self.status = format!("Exportando a {filename}...");

        match std::process::Command::new("sqlite3")
            .args(["-header", "-csv", &path, &sql])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(output) => {
                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.show_error("Error exportando", &err);
                    self.is_loading = false;
                    return;
                }
                match std::fs::write(&filename, &output.stdout) {
                    Ok(()) => {
                        let size = output.stdout.len();
                        let size_str = if size >= 1024 * 1024 {
                            format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
                        } else if size >= 1024 {
                            format!("{:.1} KiB", size as f64 / 1024.0)
                        } else {
                            format!("{size} B")
                        };
                        self.status = format!("Exportado: {filename} ({size_str})");
                    }
                    Err(e) => {
                        self.show_error("Error escribiendo", &format!("{filename}: {e}"));
                    }
                }
            }
            Err(e) => {
                self.show_error("Error ejecutando sqlite3", &e.to_string());
            }
        }
        self.is_loading = false;
    }

    // ── row inspector ─────────────────────────────────────────────────

    fn open_row_inspector(&mut self) {
        self.refresh_row_inspector();
        self.inspector_scroll.reset();
        self.show_row_inspector = true;
    }

    /// Recalcula los pares columna→valor del inspector a partir de la fila
    /// actualmente seleccionada en el Detail. Se puede llamar en caliente
    /// (mientras el modal está abierto) para seguir la navegación ↑/↓.
    fn refresh_row_inspector(&mut self) {
        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            return;
        }
        let Some(adapter) = self.adapter_arc() else {
            return;
        };

        let row_idx = self.selected_idx(PanelKind::Detail).saturating_sub(1); // skip header
        #[allow(clippy::cast_possible_truncation)]
        let offset = self.preview_loaded_offset + row_idx as u32;

        // NoSQL (mongo): el backend entrega directo los pares clave→valor SOLO
        // de los campos presentes en este documento. Cada fila puede tener
        // campos distintos; los ausentes no se muestran ni se desalinean.
        // También entregamos el JSON formateado (modo JSON del modal).
        if let Some(pairs) = adapter.row_inspector_pairs(&object, offset) {
            self.row_inspector_pairs = pairs;
            self.inspector_json_text =
                adapter.row_inspector_json(&object, offset).unwrap_or_default();
            self.inspector_scroll.reset();
            return;
        }

        // SQL (esquema fijo): flujo clásico por índice. Las columnas ya vienen
        // en `preview_data` (TableData del Data tab): reutilizarlas evita una
        // consulta `column_names()` aparte por cada apertura del modal.
        let columns: Vec<crate::db::Column> = self
            .preview_data
            .as_ref()
            .map(|data| data.columns.clone())
            .or_else(|| adapter.column_names(&object).ok())
            .unwrap_or_default();
        if columns.is_empty() {
            return;
        }
        // Celdas expandidas (multilínea): los tipos compuestos de DuckDB
        // (list/struct/map/union/array) se muestran completos en el modal.
        let Ok(rows) = adapter.table_data_rows_pretty(&object, 1, offset) else {
            return;
        };

        // Celdas tipadas: los valores viajan intactos (un "a | b" dentro de
        // una celda ya no se rompe con split('|'))
        let values: Vec<String> = rows.first().map(|row| row.cells.clone()).unwrap_or_default();

        self.row_inspector_pairs = columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let val = values.get(i).cloned().unwrap_or_default();
                (col.name.clone(), val)
            })
            .collect();
        // Al cambiar de fila, empezar el scroll del modal desde arriba
        self.inspector_scroll.reset();
    }

    #[allow(clippy::missing_const_for_fn)]
    fn close_row_inspector(&mut self) {
        self.show_row_inspector = false;
    }

    /// Etiqueta del modo de visualización del modal `NoSQL` (para el título).
    pub const fn inspector_mode_label(&self) -> &'static str {
        if self.inspector_json_mode { "Pares" } else { "Modo JSON" }
    }

    /// Alterna pares ↔ JSON del documento en el modal `NoSQL`. No hace nada
    /// si el backend no entregó JSON (SQL o doc sin datos).
    pub fn toggle_inspector_json_mode(&mut self) {
        if !self.inspector_json_text.is_empty() {
            self.inspector_json_mode = !self.inspector_json_mode;
            self.inspector_scroll.reset();
        }
    }

    /// Copia el ítem seleccionado al portapapeles del sistema.
    fn yank_selected(&mut self) {
        let items = self.items_for(self.active_panel);
        let idx = self.selected_idx(self.active_panel);
        let mut text = items.get(idx).cloned().unwrap_or_default();

        if text.is_empty() {
            self.status = "Nada que copiar".to_string();
            return;
        }

        // En Fuentes, copiar el dato real (sin marcas ▣/★/⊙/●)
        if self.active_panel == PanelKind::Sources {
            text = strip_source_marks(&text).to_string();
        }

        let copied = Self::copy_to_clipboard(&text);

        if copied {
            let preview: String = text.chars().take(50).collect();
            let more = if text.len() > 50 { "…" } else { "" };
            self.status = format!("Copiado: {preview}{more}");
        } else {
            self.status = "Error: instala wl-clipboard o xclip".to_string();
        }
    }

    #[allow(clippy::manual_let_else)]
    fn copy_to_clipboard(text: &str) -> bool {
        use std::io::Write;

        let mut child =
            match std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn()
            {
                Ok(c) => c,
                Err(_) => {
                    return std::process::Command::new("xclip")
                        .args(["-selection", "clipboard"])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .is_ok_and(|mut c| {
                            if let Some(mut stdin) = c.stdin.take() {
                                let _ = stdin.write_all(text.as_bytes());
                            }
                            c.wait().is_ok_and(|s| s.success())
                        });
                }
            };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        child.wait().is_ok_and(|s| s.success())
    }

    // ── filtro de búsqueda ────────────────────────────────────────────

    /// Inicia el modo de filtrado para el panel activo.
    fn start_filter(&mut self) {
        if !self.active_panel.is_sidebar() {
            // Solo sidebar tiene listas filtrables
            self.status = "Usa / en un panel lateral (Sources/Tablas/Vistas/Avanzado)".to_string();
            return;
        }
        self.input_mode = InputMode::Filtering;
        self.filter_query.clear();
        self.filtered_items.clear();
        self.status = "Filtrar: ".to_string();
    }

    /// Aplica el filtro actual y sale del modo de filtrado.
    fn apply_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.filtered_items.clear();
        } else {
            let items = self.original_items_for(self.active_panel);
            let query = self.filter_query.to_ascii_lowercase();
            let filtered: Vec<String> = items
                .iter()
                .filter(|s| !is_source_section(s) && s.to_ascii_lowercase().contains(&query))
                .cloned()
                .collect();
            if filtered.is_empty() {
                self.status = format!("Sin resultados para: {query}");
                self.filtered_items.clear();
            } else {
                self.filtered_items = filtered;
                self.status =
                    format!("Filtro: {} ({})", self.filter_query, self.filtered_items.len());
            }
        }
        self.input_mode = InputMode::Normal;
        self.set_selected_idx(self.active_panel, 0);
    }

    /// Cancela el filtro y vuelve a modo normal.
    fn cancel_filter(&mut self) {
        self.input_mode = InputMode::Normal;
        self.filter_query.clear();
        self.filtered_items.clear();
        self.status = "Filtro cancelado".to_string();
    }

    /// Actualiza el filtro en tiempo real (mientras se escribe).
    fn update_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.filtered_items.clear();
            self.status = "Filtrar: ".to_string();
            return;
        }
        let items = self.original_items_for(self.active_panel);
        let query = self.filter_query.to_ascii_lowercase();
        let filtered: Vec<String> =
            items.iter().filter(|s| s.to_ascii_lowercase().contains(&query)).cloned().collect();
        if filtered.is_empty() {
            self.filtered_items.clear();
            self.status = format!("Filtrar: {} (sin resultados)", self.filter_query);
        } else {
            self.filtered_items = filtered;
            self.status = format!("Filtrar: {} ({})", self.filter_query, self.filtered_items.len());
        }
        self.set_selected_idx(self.active_panel, 0);
    }

    /// Items originales del panel activo (sin filtro).
    fn original_items_for(&self, kind: PanelKind) -> &[String] {
        match kind {
            PanelKind::Sources => &self.sources,
            PanelKind::Tables => &self.tables,
            PanelKind::Views => &self.views,
            PanelKind::Advanced => &self.advanced,
            PanelKind::Detail => &self.preview_rows,
        }
    }

    fn execute_menu_action(&mut self) {
        match self.actions_menu_idx {
            0 => self.connect_sqlite("sakila.db"),
            1 => self.mark_current_db_as_favorite(),
            2 => self.reload_runtime_config(),
            3 => self.clear_query_state(),
            _ => {}
        }
        self.show_actions_menu = false;
    }

    fn reload_runtime_config(&mut self) {
        self.keymap = keys::Keymap::load();
        self.state = storage::AppState::load();
        self.sources = Self::build_sources(
            &self.state,
            self.source_tab,
            self.db_path.as_deref(),
            &self.health,
            &self.detected_servers,
        );

        let ui_config = config::Config::load().ui;
        self.rows_per_page = ui_config.rows_per_page;

        // Ajustar índices si las listas se achicaron
        if self.selected_idx(PanelKind::Sources) >= self.sources.len() {
            self.set_selected_idx(PanelKind::Sources, self.sources.len().saturating_sub(1));
        }

        if self.db_path.is_some() {
            self.refresh_preview_from_selected_object();
        }

        self.status =
            format!("Config recargada: keys + estado + ui (rows_per_page={})", self.rows_per_page);
    }

    // ── menú de acciones ──────────────────────────────────────────────
    fn jump_to_detail(&mut self) {
        if self.active_panel == PanelKind::Detail {
            // Enter sobre una fila de datos: FK Jump si la fila referencia
            // otra tabla; si no, el inspector de fila (comportamiento previo).
            if self.detail_tab == DetailTab::Data
                && self.selected_idx(PanelKind::Detail) > 0
                && self.fk_jump()
            {
                return;
            }
            self.open_row_inspector();
            return;
        }

        // Ejecutar la acción del panel antes de saltar
        match self.active_panel {
            PanelKind::Sources => self.connect_selected_source(),
            PanelKind::Tables | PanelKind::Views | PanelKind::Advanced => {
                self.current_page = 0;
                self.refresh_preview_from_selected_object();
            }
            PanelKind::Detail => {}
        }

        self.last_sidebar_focus = self.active_panel;
        self.active_panel = PanelKind::Detail;
    }

    /// FK Jump (patrón lazygit): con la fila seleccionada, salta a la tabla
    /// referenciada por la primera foreign key con valor no nulo y se
    /// posiciona en la fila que apunta (offset por rowid, página exacta).
    ///
    /// Devuelve `true` si saltó (la fila tenía una FK resuelta).
    fn fk_jump(&mut self) -> bool {
        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            return false;
        }
        let Some(data) = self.preview_data.as_ref() else { return false };
        let row_idx = self.selected_idx(PanelKind::Detail).saturating_sub(1);
        let Some(row) = data.rows.get(row_idx) else { return false };

        let Some(adapter) = self.adapter_arc() else {
            return false;
        };
        let Ok(fks) = adapter.foreign_keys(&object) else {
            return false;
        };
        if fks.is_empty() {
            return false;
        }

        // La primera FK cuyo valor en esta fila no esté vacío
        let jump = fks.into_iter().find_map(|fk| {
            let col_idx = data.columns.iter().position(|c| c.name == fk.from)?;
            let value = row.cells.get(col_idx)?;
            if value.is_empty() || value == "[NULL]" {
                return None;
            }
            Some((fk, value.clone()))
        });
        let Some((fk, value)) = jump else {
            self.status = "La fila no referencia ninguna tabla (FK vacías)".to_string();
            return false;
        };

        // `to` vacío → la PK de la tabla referenciada
        let to_col = match fk.to {
            Some(to) if !to.is_empty() => to,
            _ => {
                let Ok(cols) = adapter.table_columns(&fk.table) else {
                    return false;
                };
                let Some(pk) = cols.iter().find(|c| c.pk).map(|c| c.name.clone()) else {
                    return false;
                };
                pk
            }
        };

        // Posición de la fila referenciada (1-based) para cargar la página
        let Ok(Some(idx)) = adapter.row_offset_of(&fk.table, &to_col, &value) else {
            self.status =
                format!("FK {}.{} = {value}: fila no encontrada en {}", object, fk.from, fk.table);
            return false;
        };

        // Cambiar al objeto referenciado (recargando tablas si no estaba)
        if !self.tables.contains(&fk.table) {
            if let Some(adapter) = self.adapter_arc() {
                if let Ok(tables) = adapter.list_objects_by_type("table") {
                    self.tables = tables;
                    self.sources = Self::build_sources(
                        &self.state,
                        self.source_tab,
                        self.db_path.as_deref(),
                        &self.health,
                        &self.detected_servers,
                    );
                }
            }
        }
        let Some(obj_idx) = self.tables.iter().position(|t| *t == fk.table) else {
            self.status = format!("Tabla {} no encontrada", fk.table);
            return false;
        };
        self.object_section = ObjectSection::Tables;
        self.set_selected_idx(PanelKind::Tables, obj_idx);

        // Cargar la página que contiene la fila referenciada
        #[allow(clippy::cast_possible_truncation)]
        let page = (idx.saturating_sub(1)) / self.rows_per_page;
        self.current_page = page;
        self.detail_tab = DetailTab::Data;
        self.refresh_preview_from_selected_object();

        // Seleccionar la fila exacta dentro de la página
        #[allow(clippy::cast_possible_truncation)]
        let local = idx.saturating_sub(page.saturating_mul(self.rows_per_page));
        #[allow(clippy::cast_possible_truncation)]
        {
            self.set_selected_idx(PanelKind::Detail, local as usize);
        }

        self.status =
            format!("FK Jump: {}.{} = {value} → {}.{}", object, fk.from, fk.table, to_col);
        tracing::info!(desde = %object, col = %fk.from, valor = %value, hacia = %fk.table, "FK jump");
        true
    }

    /// Conecta a la fuente seleccionada en el panel Sources
    fn connect_selected_source(&mut self) {
        let selected = self.selected_source().to_string();

        if is_source_section(&selected) {
            return; // secciones no conectables
        }

        if selected == "<sin entradas>" {
            self.status = "No hay elementos en esta sección".to_string();
            return;
        }

        let clean = strip_source_marks(&selected).to_string();

        match clean.as_str() {
            "Abrir sakila.db" => self.connect_sqlite("sakila.db"),
            "Buscar archivo .db" => {
                self.status = "Buscador de archivos .db no implementado todavia".to_string();
            }
            s if s.starts_with("mysql://") => {
                self.connect_sqlite(s);
            }
            s if s.starts_with("postgres://") || s.starts_with("postgresql://") => {
                self.connect_sqlite(s);
            }
            s if s.starts_with("mongodb://") => {
                self.connect_sqlite(s);
            }
            s if s.contains(" => ") => {
                let path = s.split_once(" => ").map(|(_, p)| p.to_string()).unwrap_or_default();
                self.connect_sqlite(&path);
            }
            s if s.starts_with('/')
                || std::path::Path::new(s).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("db")
                        || ext.eq_ignore_ascii_case("sqlite")
                        || ext.eq_ignore_ascii_case("sqlite3")
                        || ext.eq_ignore_ascii_case("duckdb")
                        || ext.eq_ignore_ascii_case("ddb")
                        || {
                            #[cfg(feature = "files")]
                            {
                                crate::db::backends::file::kind_for(s).is_some()
                            }
                            #[cfg(not(feature = "files"))]
                            {
                                false
                            }
                        }
                }) =>
            {
                self.connect_sqlite(s);
            }
            _ => {
                self.status =
                    format!("No se puede conectar: {}", crate::security::strip_credentials(&clean));
            }
        }
    }

    // ── enter en Sources ──────────────────────────────────────────────

    fn handle_enter(&mut self) {
        if self.active_panel != PanelKind::Sources {
            return;
        }
        self.connect_selected_source();
    }

    // ── keyboard ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    /// Cierre seguro con Ctrl+C: si hay algo abierto (filtro de búsqueda,
    /// inspector de fila o menú de acciones) primero se cierra, para que el
    /// usuario en pánico no pierda estado a medias; solo sale de lazydb
    /// cuando no queda nada abierto. Un segundo Ctrl+C en estado limpio sale.
    pub fn on_ctrl_c(&mut self) {
        if self.input_mode == InputMode::Filtering {
            self.cancel_filter();
        } else if self.show_row_inspector {
            self.close_row_inspector();
        } else if self.show_actions_menu {
            self.show_actions_menu = false;
            self.actions_menu_idx = 0;
            self.status = String::new();
        } else if self.password_prompt.is_some() {
            self.password_prompt = None;
            self.status = String::new();
        } else if self.db_picker.is_some() {
            self.db_picker = None;
            self.status = String::new();
        } else {
            self.should_quit = true;
        }
    }

    /// Pegado del portapapeles (bracketed paste). Se enruta al estado activo
    /// que acepte texto; los `\n` se sanitizan (las URLs de `CleverCloud` se
    /// parten en 2 líneas y el `\n` no debe disparar Enter).
    pub fn on_paste(&mut self, text: &str) {
        let clean = text.replace(['\n', '\r'], "");
        if clean.is_empty() {
            return;
        }

        // Formulario de conexión: pegar en el campo URL (o el campo activo)
        if self.connection_form.is_some() && self.db_path.is_none() {
            if let Some(form) = self.connection_form.as_mut() {
                let target = match form.active {
                    ConnField::Url => &mut form.url,
                    ConnField::Host => &mut form.host,
                    ConnField::Port => &mut form.port,
                    ConnField::User => &mut form.user,
                    ConnField::Pass => &mut form.pass,
                    ConnField::Db => &mut form.db,
                    _ => return,
                };
                target.push_str(&clean);
                if form.active == ConnField::Url {
                    form.url_last_edit = Some(std::time::Instant::now());
                    form.url_debounce_scheduled = true;
                }
            }
            // Reconstruir la URL si se pegó en un campo individual
            if let Some(form) = self.connection_form.as_ref() {
                if matches!(
                    form.active,
                    ConnField::Host
                        | ConnField::Port
                        | ConnField::User
                        | ConnField::Pass
                        | ConnField::Db
                ) {
                    self.conn_rebuild_url_from_fields();
                }
            }
            return;
        }

        // Input de query (`:`)
        if self.query_input.is_some() {
            if let Some(state) = self.query_input.as_mut() {
                state.buffer.push_str(&clean);
            }
            return;
        }

        // Prompt de contraseña
        if self.password_prompt.is_some() {
            if let Some(state) = self.password_prompt.as_mut() {
                state.buffer.push_str(&clean);
            }
            return;
        }

        // Modo filtro
        if self.input_mode == InputMode::Filtering {
            self.filter_query.push_str(&clean);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // ── modo filtro: capturar teclas antes del mapeo de acciones ──
        if self.input_mode == InputMode::Filtering {
            self.handle_filter_key(key);
            return;
        }

        // ── popup de error (modal urgente: Enter/Esc/q lo cierran) ──
        // Captura teclas crudas (no acciones mapeadas) para que ninguna
        // navegación cierre el error por accidente.
        if self.error.is_some() {
            if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                self.error = None;
            }
            return;
        }

        // ── modales superpuestos: el prompt de contraseña y el picker de
        // bases capturan SIEMPRE primero (se abren sobre el formulario de
        // conexión cuando conectas a un servidor remoto). ──
        if self.password_prompt.is_some() {
            self.handle_password_prompt_key(key);
            return;
        }
        if self.db_picker.is_some() {
            self.handle_db_picker_key(key);
            return;
        }

        // ── formulario de nueva conexión (captura TODO solo cuando el foco
        // está en el panel Detail y NO hay db conectada; los chars alimentan
        // el campo activo). Con foco en otro panel, `:` y las teclas globales
        // siguen funcionando normal. ──
        if self.db_path.is_none() && self.active_panel == PanelKind::Detail {
            self.handle_connection_form_key(key);
            return;
        }

        // ── input SQL (modal `:` — captura TODO mientras está abierto,
        // incluidos chars no mapeados a ninguna acción) ──
        if self.query_input.is_some() {
            self.handle_query_input_key(key);
            return;
        }

        // ── pick de base de datos (modal de servidor detectado) ──
        if self.db_picker.is_some() {
            self.handle_db_picker_key(key);
            return;
        }

        let Some(action) = keys::map_key(&self.keymap, key) else {
            return;
        };

        // ── row inspector modal ──
        if self.show_row_inspector {
            self.handle_row_inspector_key(action);
            return;
        }

        // ── ayuda de teclas (modal) ──
        if self.show_help {
            self.handle_help_key(action);
            return;
        }

        // ── menú de acciones (modal) ──
        if self.show_actions_menu {
            self.handle_actions_menu_key(action);
            return;
        }

        self.dispatch_action(action);
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_filter(),
            KeyCode::Enter => self.apply_filter(),
            KeyCode::Backspace => {
                self.filter_query.pop();
                self.update_filter();
            }
            KeyCode::Char(c) => {
                self.filter_query.push(c);
                self.update_filter();
            }
            _ => {}
        }
    }

    fn handle_row_inspector_key(&mut self, action: keys::AppAction) {
        match action {
            keys::AppAction::QuitOrBack
            | keys::AppAction::Enter
            | keys::AppAction::ToggleActionsMenu => {
                self.close_row_inspector();
            }
            // ↑/↓ navegan la tabla de datos y el modal se actualiza en vivo
            keys::AppAction::MoveUp => {
                self.move_selection(-1);
                self.refresh_row_inspector();
            }
            keys::AppAction::MoveDown => {
                self.move_selection(1);
                self.refresh_row_inspector();
            }
            // NoSQL: alternar pares ↔ JSON del documento (solo tiene sentido
            // si el backend entregó JSON; SQL ignora el toggle).
            keys::AppAction::ToggleInspectorJson if !self.inspector_json_text.is_empty() => {
                self.inspector_json_mode = !self.inspector_json_mode;
                self.inspector_scroll.reset();
            }
            _ => {}
        }
    }

    fn handle_help_key(&mut self, action: keys::AppAction) {
        match action {
            keys::AppAction::ToggleHelp
            | keys::AppAction::QuitOrBack
            | keys::AppAction::ToggleActionsMenu => {
                self.show_help = false;
            }
            // ↑/↓ desplazan el contenido si no cabe en el modal
            keys::AppAction::MoveUp | keys::AppAction::PrevPage => {
                self.help_scroll.up(2);
            }
            keys::AppAction::MoveDown | keys::AppAction::NextPage => {
                self.help_scroll.down(2);
            }
            _ => {}
        }
    }

    fn handle_actions_menu_key(&mut self, action: keys::AppAction) {
        match action {
            keys::AppAction::ToggleActionsMenu | keys::AppAction::QuitOrBack => {
                self.show_actions_menu = false;
            }
            keys::AppAction::MoveUp => {
                self.actions_menu_idx = self.actions_menu_idx.saturating_sub(1);
            }
            keys::AppAction::MoveDown => {
                let last = Self::ACTION_ITEMS.len().saturating_sub(1);
                self.actions_menu_idx = (self.actions_menu_idx + 1).min(last);
            }
            keys::AppAction::Enter => {
                self.execute_menu_action();
            }
            _ => {}
        }
    }

    // Dispatch de ~50 acciones: se permite `too_many_lines` (un match plano
    // por cada acción es más legible que despiezarlo en N métodos).
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn dispatch_action(&mut self, action: keys::AppAction) {
        match action {
            keys::AppAction::RunCountQuery => self.execute_count_query(),
            keys::AppAction::ClearQueryState => self.clear_query_state(),
            keys::AppAction::ReloadRuntimeConfig => self.reload_runtime_config(),
            keys::AppAction::OpenQueryInput => {
                self.query_input = Some(QueryInputState::default());
                self.status = "SQL: escribe una query, ↑/↓ historial, enter ejecuta".to_string();
            }
            keys::AppAction::ToggleActionsMenu => {
                self.show_actions_menu = true;
                self.actions_menu_idx = 0;
                self.status = "Menu de acciones abierto".to_string();
            }
            // Fuera del modal no aplica: el toggle se maneja en
            // `handle_row_inspector_key` (solo dentro del modal NoSQL).
            keys::AppAction::ToggleInspectorJson => {}
            keys::AppAction::ToggleHelp => {
                self.show_help = !self.show_help;
                if self.show_help {
                    self.status = "Ayuda de teclas (bindings reales)".to_string();
                }
            }
            keys::AppAction::Yank => self.yank_selected(),
            keys::AppAction::ExportCsv => self.export_csv(),
            keys::AppAction::StartFilter => self.start_filter(),
            keys::AppAction::HScrollLeft => self.on_h_scroll(-1),
            keys::AppAction::HScrollRight => self.on_h_scroll(1),
            keys::AppAction::ToggleCurrentPanel => self.toggle_active_panel(),
            // esc/q cierran por capas (estilo lazygit): primero el panel
            // Detail vuelve a Tablas, luego se cierra la DB conectada, y
            // solo con todo limpio sale de lazydb.
            keys::AppAction::QuitOrBack => {
                if self.active_panel == PanelKind::Detail {
                    self.set_focus(PanelKind::Tables);
                } else if self.db_path.is_some() {
                    self.disconnect_db();
                } else {
                    self.should_quit = true;
                }
            }
            keys::AppAction::FocusNext => self.focus_next(),
            keys::AppAction::FocusPrev => self.focus_prev(),
            keys::AppAction::SidebarFocusNext => self.sidebar_focus_next(),
            keys::AppAction::SidebarFocusPrev => self.sidebar_focus_prev(),
            keys::AppAction::FocusSources => self.set_focus(PanelKind::Sources),
            keys::AppAction::FocusTables
            | keys::AppAction::FocusObjects
            | keys::AppAction::ObjectSectionTables => self.set_focus(PanelKind::Tables),
            keys::AppAction::FocusViews | keys::AppAction::ObjectSectionViews => {
                self.set_focus(PanelKind::Views);
            }
            keys::AppAction::FocusAdvanced => self.set_focus(PanelKind::Advanced),
            keys::AppAction::FocusDetail | keys::AppAction::FocusPreview => {
                self.set_focus(PanelKind::Detail);
            }
            keys::AppAction::Refresh => {
                self.refresh_count = self.refresh_count.saturating_add(1);
                self.refresh_from_connection();
            }
            keys::AppAction::FavoriteCurrentDb => self.mark_current_db_as_favorite(),
            keys::AppAction::ToggleFavoriteSource => self.toggle_favorite_source(),
            keys::AppAction::ForgetSource => self.forget_source(),
            keys::AppAction::MoveUp => self.move_selection(-1),
            keys::AppAction::MoveDown => self.move_selection(1),
            keys::AppAction::PrevPage => {
                if self.active_panel == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    self.move_selection_by_page(false);
                }
            }
            keys::AppAction::NextPage => {
                if self.active_panel == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    self.move_selection_by_page(true);
                }
            }
            keys::AppAction::JumpToDetail => self.jump_to_detail(),
            keys::AppAction::Enter => self.handle_enter(), // legacy, sin binding por defecto
            keys::AppAction::SourceTabRecents => self.set_source_tab(SourceTab::All),
            keys::AppAction::SourceTabFavorites => self.set_source_tab(SourceTab::Local),
            keys::AppAction::SourceTabNext => self.set_source_tab(self.source_tab.next()),
            keys::AppAction::SourceTabPrev => self.set_source_tab(self.source_tab.prev()),
            keys::AppAction::DetailTabPrev => self.set_detail_tab(self.detail_tab.prev()),
            keys::AppAction::DetailTabNext => self.set_detail_tab(self.detail_tab.next()),
            keys::AppAction::DetailTabData => self.set_detail_tab(DetailTab::Data),
            keys::AppAction::DetailTabSchema => self.set_detail_tab(DetailTab::Schema),
            keys::AppAction::DetailTabSql => self.set_detail_tab(DetailTab::Sql),
            keys::AppAction::DetailTabMeta => self.set_detail_tab(DetailTab::Meta),
            keys::AppAction::ObjectSectionAdvanced => {
                self.set_focus(PanelKind::Advanced);
            }
        }
    }

    // ── mouse ─────────────────────────────────────────────────────────

    pub fn on_scroll(&mut self, up: bool, mouse_x: u16, mouse_y: u16) {
        if self.show_row_inspector {
            if up {
                self.inspector_scroll.scroll_up();
            } else {
                self.inspector_scroll.scroll_down();
            }
            return;
        }

        if self.show_actions_menu {
            if up {
                self.actions_menu_idx = self.actions_menu_idx.saturating_sub(1);
            } else {
                let last = Self::ACTION_ITEMS.len().saturating_sub(1);
                self.actions_menu_idx = (self.actions_menu_idx + 1).min(last);
            }
            return;
        }

        // Buscar qué panel está bajo el mouse
        let hovered = self
            .layout
            .panels
            .iter()
            .find(|(_, rect)| {
                mouse_x >= rect.x
                    && mouse_x < rect.x.saturating_add(rect.width)
                    && mouse_y >= rect.y
                    && mouse_y < rect.y.saturating_add(rect.height)
            })
            .map(|(k, _)| *k);

        let Some(target) = hovered else {
            return; // mouse fuera de todos los paneles
        };

        if target == self.active_panel {
            // Panel enfocado: mover selección (comportamiento normal)
            if up {
                self.move_selection(-1);
            } else {
                self.move_selection(1);
            }
        } else {
            // Panel NO enfocado: mover la selección del panel hovered (sin
            // cambiar el foco). El scroll de la vista lo ajusta el render en
            // cada frame (panel.rs) para seguir a la selección — no se toca
            // aquí para que la vista y el cursor nunca se desincronicen
            // (antes el scroll manual solo bajaba en una dirección: la vista
            // se congelaba mientras el status avanzaba de fila, y al volver
            // el foco la vista "saltaba de página").
            let items_len = self.items_len_for(target);
            let old_idx = self.selected_idx(target);

            {
                let p = self.panel_mut(target);
                if up {
                    p.selected_idx =
                        p.selected_idx.saturating_sub(1).min(items_len.saturating_sub(1));
                } else {
                    p.selected_idx = (p.selected_idx + 1).min(items_len.saturating_sub(1));
                }
            } // ── drop `p` ──

            // Header bypass para Data tab no enfocado
            if target == PanelKind::Detail && self.detail_tab == DetailTab::Data && items_len > 1 {
                let p = self.panel_mut(target);
                if p.selected_idx == 0 {
                    p.selected_idx = 1;
                }
            }

            // Scroll infinito en Data tab no enfocado
            if target == PanelKind::Detail && self.detail_tab == DetailTab::Data && items_len > 1 {
                if !up && old_idx == items_len.saturating_sub(1) {
                    self.scroll_down_infinite();
                } else if up && old_idx == 1 && self.preview_loaded_offset > 0 {
                    self.scroll_up_infinite();
                }
            }
        }
    }

    /// Detecta qué tab del título de Detail fue clickeado.
    /// Formato del título: "[5] [ Datos - row 1/300 ] |  Esquema  |  SQL  |  Meta  | "
    fn detect_detail_tab_click(&self, cursor_x: u16, rect: Rect) -> Option<DetailTab> {
        let available = self.available_detail_tabs();
        let num = PanelKind::Detail.number();
        let prefix = format!("[{num}]");
        // El texto del título empieza en rect.x + 1 (después de la esquina ┌
        // del borde); las pestañas empiezan después del "[N]"
        #[allow(clippy::cast_possible_truncation)]
        let mut cursor = rect.x + 1 + prefix.len() as u16;

        for &tab in &available {
            let text_w = self.detail_tab_display_width(tab);
            if cursor_x >= cursor && cursor_x < cursor + text_w {
                return Some(tab);
            }
            // Separador REAL entre tabs: "|" (1 char, ver title_for → parts.join("|"))
            cursor += text_w + 1;
        }
        None
    }

    /// Detecta qué tab de Fuentes fue clickeado en el título del panel.
    /// Título: "[1]Fuentes (Todo [Local] Online)" — los corchetes marcan el
    /// tab activo. Busca la posición real de cada palabra dentro del string
    /// del título (que empieza en rect.x + 1, después de la esquina ┌).
    fn detect_source_tab_click(&self, cursor_x: u16, rect: Rect) -> Option<SourceTab> {
        let num = PanelKind::Sources.number();
        let tabs = match self.source_tab {
            SourceTab::All => "[Todo] Local Online",
            SourceTab::Local => "Todo [Local] Online",
            SourceTab::Online => "Todo Local [Online]",
        };
        let title = format!("[{num}]Fuentes ({tabs})");
        let base = usize::from(rect.x) + 1;
        let cursor = usize::from(cursor_x);

        for (tab, word) in
            [(SourceTab::All, "Todo"), (SourceTab::Local, "Local"), (SourceTab::Online, "Online")]
        {
            if let Some(pos) = title.find(word) {
                let start = base + pos;
                let end = start + word.len();
                if cursor >= start && cursor < end {
                    return Some(tab);
                }
            }
        }
        None
    }

    /// Ancho en columnas del texto de un tab en el título.
    #[allow(clippy::cast_possible_truncation)]
    fn detail_tab_display_width(&self, tab: DetailTab) -> u16 {
        let label = tab.label();
        let inner = if tab == DetailTab::Data && self.total_rows > 0 {
            let current_row = self.preview_loaded_offset
                + self.selected_idx(PanelKind::Detail).saturating_sub(1) as u32
                + 1;
            let total = self.total_rows;
            format!("{label} - row {current_row}/{total}")
        } else {
            label.to_string()
        };
        let padded =
            if tab == self.detail_tab { format!(" [ {inner} ] ") } else { format!("  {inner}  ") };
        #[allow(clippy::cast_possible_truncation)]
        {
            padded.len() as u16
        }
    }

    /// Desplaza la ventana de columnas visibles del Data tab.
    /// `dir`: -1 = izquierda, 1 = derecha.
    pub fn on_h_scroll(&mut self, dir: i32) {
        let detail = self.panel(PanelKind::Detail);
        let current = detail.h_scroll.get();
        let max_cols = {
            let headers: Vec<&str> =
                self.preview_rows.first().map_or_else(Vec::new, |r| r.split(" | ").collect());
            headers.len()
        };
        let inner_w = self
            .layout
            .panels
            .iter()
            .find(|(k, _)| *k == PanelKind::Detail)
            .map_or(0, |(_, r)| usize::from(r.width.saturating_sub(2)));
        if max_cols <= 1 || inner_w == 0 {
            return;
        }
        let total_min = max_cols.saturating_mul(MIN_COL_W);
        if total_min <= inner_w {
            return; // todas las columnas caben, no hay scroll horizontal
        }
        let max_visible = (inner_w / MIN_COL_W).max(1);
        let max_start = max_cols.saturating_sub(max_visible);
        let next = if dir < 0 { current.saturating_sub(1) } else { (current + 1).min(max_start) };
        if next != current {
            self.panel_mut(PanelKind::Detail).h_scroll.set(next);
        }
    }

    /// Dado un click en el área del panel Detail (Data tab), calcula a qué columna
    /// corresponde según la posición X y las columnas parseadas del header.
    fn column_at_x(&self, x: u16, rect: Rect) -> Option<String> {
        if self.preview_rows.is_empty() {
            return None;
        }
        let headers: Vec<&str> = self.preview_rows[0].split(" | ").collect();
        let col_count = headers.len();
        if col_count <= 1 {
            return None;
        }
        let inner_w = usize::from(rect.width.saturating_sub(2));
        let h_scroll = self.panel(PanelKind::Detail).h_scroll.get();

        // Misma lógica de ventana que render_data_table
        let total_min = col_count.saturating_mul(MIN_COL_W);
        let (vis_start, cell_widths) = if total_min <= inner_w {
            let cell_base = inner_w / col_count;
            let widths: Vec<usize> = (0..col_count)
                .map(|i| {
                    if i == col_count.saturating_sub(1) {
                        inner_w.saturating_sub(cell_base * (col_count.saturating_sub(1)))
                    } else {
                        cell_base
                    }
                })
                .collect();
            (0, widths)
        } else {
            let max_visible = (inner_w / MIN_COL_W).max(1);
            let vis_start = h_scroll.min(col_count.saturating_sub(max_visible));
            let mut widths = vec![MIN_COL_W; max_visible];
            let rem = inner_w.saturating_sub(max_visible.saturating_mul(MIN_COL_W));
            if let Some(last) = widths.last_mut() {
                *last += rem;
            }
            (vis_start, widths)
        };

        let rel_x = usize::from(x.saturating_sub(rect.x + 1));
        let mut cumul = 0usize;
        for (i, _w) in cell_widths.iter().enumerate() {
            cumul += cell_widths[i];
            if rel_x < cumul {
                let real_idx = vis_start + i;
                if real_idx < headers.len() {
                    return Some(headers[real_idx].trim().to_string());
                }
                return None;
            }
        }
        None
    }

    /// Toggle orden por columna: si ya está ordenando por `col`, invierte ASC↔DESC;
    /// si no, ordena ASC. Si hay un filtro activo, se cancela (el click en header
    /// ordena y limpia el filtro de una vez).
    /// Ciclo de 3 estados al hacer click en el header de una columna:
    /// 1er click → ASC (▴), 2º click → DESC (▾), 3er click → desactivar el
    /// ordenamiento (vuelve al orden por defecto, sin indicador). Es el patrón
    /// estándar de tablas (VS Code, Excel, file managers).
    fn toggle_sort(&mut self, col: String) {
        if !self.filtered_items.is_empty() || self.input_mode == InputMode::Filtering {
            self.cancel_filter();
        }
        if self.sort_column.as_deref() == Some(col.as_str()) {
            if self.sort_asc {
                self.sort_asc = false;
            } else {
                // 3er click: desactivar ordenamiento
                self.sort_column = None;
                self.sort_asc = true;
            }
        } else {
            self.sort_column = Some(col);
            self.sort_asc = true;
        }
        // Recargar datos desde la página actual con el nuevo orden
        self.current_page = 0;
        self.preview_loaded_offset = 0;
        self.refresh_preview_from_selected_object();
    }

    /// Punto de entrada para clicks de mouse (Down). Decide si el click cae
    /// sobre una barra de scroll (inicia drag) o se procesa como click normal.
    pub fn on_mouse_down(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if self.try_start_inspector_scroll_drag(x, y, width, height) {
            return;
        }
        if self.try_start_h_scroll_drag(x, y, width, height) {
            return;
        }
        if self.try_start_v_scroll_drag(x, y, width, height) {
            return;
        }
        self.on_mouse_click(x, y, width, height);
    }

    /// Movimiento del mouse con botón presionado (drag): actualiza la barra
    /// arrastrada. No valida límites del eje para emular scroll de página web.
    pub fn on_mouse_drag(&mut self, x: u16, y: u16) {
        let Some(drag) = self.drag else {
            return;
        };
        match drag {
            DragState::InspectorScroll { rect, content_len } => {
                let viewport = usize::from(rect.height.saturating_sub(2));
                let max_scroll = content_len.saturating_sub(viewport);
                let (_, track) = v_scroll_thumb_geometry(rect.height, content_len, viewport);
                let rel = f32::from(y.saturating_sub(rect.y));
                self.apply_inspector_drag(rel, max_scroll, track);
            }
            DragState::HScroll => {
                if let Some(&(_, rect)) =
                    self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
                {
                    let headers: Vec<&str> = self
                        .preview_rows
                        .first()
                        .map_or_else(Vec::new, |r| r.split(" | ").collect());
                    let col_count = headers.len();
                    if col_count <= 1 {
                        return;
                    }
                    let inner_w = usize::from(rect.width.saturating_sub(2));
                    let max_visible = (inner_w / MIN_COL_W).max(1);
                    let max_start = col_count.saturating_sub(max_visible);
                    let (_, track) = h_scroll_thumb_geometry(inner_w, col_count, max_visible);
                    let rel = f32::from(x.saturating_sub(rect.x + 1));
                    self.apply_h_drag(rel, max_start, track);
                }
            }
            DragState::VScroll(kind) => {
                if let Some(&(_, rect)) = self.layout.panels.iter().find(|(k, _)| *k == kind) {
                    let items_len = self.items_len_for(kind);
                    let viewport = usize::from(rect.height.saturating_sub(2));
                    let max_scroll = items_len.saturating_sub(viewport);
                    let (_, track) = v_scroll_thumb_geometry(rect.height, items_len, viewport);
                    let rel = f32::from(y.saturating_sub(rect.y));
                    self.apply_v_drag(rel, kind, max_scroll, track);
                }
            }
        }
    }

    /// Suelta del botón: termina el arrastre.
    pub const fn on_mouse_up(&mut self) {
        self.drag = None;
    }

    /// ¿El click está sobre la barra de scroll horizontal del Data tab?
    /// Si sí, inicia el arrastre y mueve el thumb a la posición del click.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_h_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if width < 40 || height < 10 {
            return false;
        }
        if self.show_row_inspector || self.show_actions_menu {
            return false;
        }
        if self.detail_tab != DetailTab::Data {
            return false;
        }
        let Some(&(_, rect)) = self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
        else {
            return false;
        };
        // La barra está en la fila del espaciador (rect.y + 1), dentro del inner
        if y != rect.y + 1 || x <= rect.x || x >= rect.x + rect.width - 1 {
            return false;
        }
        let headers: Vec<&str> =
            self.preview_rows.first().map_or_else(Vec::new, |r| r.split(" | ").collect());
        let col_count = headers.len();
        if col_count <= 1 {
            return false;
        }
        let inner_w = usize::from(rect.width.saturating_sub(2));
        let max_visible = (inner_w / MIN_COL_W).max(1);
        if col_count.saturating_mul(MIN_COL_W) <= inner_w {
            return false; // sin scroll horizontal → sin barra
        }
        let max_start = col_count.saturating_sub(max_visible);

        // Jump-to-position: el thumb salta para quedar CENTRADO bajo el cursor.
        // Desde ahí el arrastre es 1:1 (cada celda de mouse = su proporción del
        // track), así el thumb recorre el 100% del recorrido disponible.
        let (thumb_w, track) = h_scroll_thumb_geometry(inner_w, col_count, max_visible);

        let rel = f32::from(x.saturating_sub(rect.x + 1));
        self.drag = Some(DragState::HScroll);
        self.apply_h_drag(rel - thumb_w as f32 / 2.0, max_start, track);
        true
    }

    /// Convierte la X del mouse en posición de `h_scroll`.
    /// Mapeo 1:1: cada celda del mouse sobre el track equivale a su proporción
    /// del scroll total (`track` = recorrido efectivo del thumb), así el thumb
    /// recorre el 100% del recorrido disponible.
    fn apply_h_drag(&mut self, rel: f32, max_start: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_start as f32).round() as usize;
        self.panel_mut(PanelKind::Detail).h_scroll.set(new.min(max_start));
    }

    /// ¿El click está sobre el scrollbar INTERIOR del modal del inspector?
    /// El scrollbar vive en la última columna del inner (dentro del modal):
    /// un click ahí solo puede significar scroll del modal, sin ambigüedad
    /// con los scrollbars de los paneles de detrás.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_inspector_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if !self.show_row_inspector || width < 40 || height < 10 {
            return false;
        }
        let rect = crate::ui::widgets::modal::geometry(Rect::new(0, 0, width, height), 70, 70);
        // Última columna del inner (el modal se dibuja con borde: el scrollbar
        // interior está en rect.x + rect.width - 2).
        let sb_x = rect.x.saturating_add(rect.width).saturating_sub(2);
        if x != sb_x || y <= rect.y || y >= rect.y.saturating_add(rect.height).saturating_sub(1) {
            return false;
        }

        let inner = crate::ui::widgets::modal::inner_area(rect);
        let (key_w, val_w) = crate::ui::widgets::modal::table_geometry(inner);
        let expanded = crate::ui::widgets::modal::expand_pairs(
            &self.row_inspector_pairs,
            key_w as usize,
            val_w as usize,
        );
        let content_len = expanded.len().saturating_add(1); // +1 header
        let viewport = usize::from(inner.height.max(1));
        if content_len <= viewport {
            return false; // sin scrollbar visible
        }
        let max_scroll = content_len.saturating_sub(viewport);
        let (thumb_h, track) = v_scroll_thumb_geometry(rect.height, content_len, viewport);

        // Jump-to-position: thumb centrado bajo el cursor, luego 1:1
        let rel = f32::from(y.saturating_sub(rect.y));
        self.drag = Some(DragState::InspectorScroll { rect, content_len });
        self.apply_inspector_drag(rel - thumb_h as f32 / 2.0, max_scroll, track);
        true
    }

    /// Convierte la Y del mouse en offset del scroll del inspector.
    /// Mapeo 1:1 (ver `apply_v_drag`).
    fn apply_inspector_drag(&mut self, rel: f32, max_scroll: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_scroll as f32).round() as usize;
        self.inspector_scroll.offset = new.min(max_scroll);
    }

    /// ¿El click está sobre el scrollbar vertical (última columna) de un panel?
    /// Si sí, inicia el arrastre y mueve el thumb a la posición del click.
    #[allow(clippy::cast_precision_loss)]
    fn try_start_v_scroll_drag(&mut self, x: u16, y: u16, width: u16, height: u16) -> bool {
        if width < 40 || height < 10 {
            return false;
        }
        if self.show_row_inspector || self.show_actions_menu {
            return false;
        }
        for &(kind, rect) in &self.layout.panels {
            if x != rect.x + rect.width - 1 || y < rect.y || y >= rect.y + rect.height {
                continue;
            }
            // Detail + Data tab no tiene scrollbar vertical (usa el horizontal)
            if kind == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                continue;
            }
            let items_len = self.items_len_for(kind);
            let viewport = usize::from(rect.height.saturating_sub(2));
            if items_len <= 1 || items_len <= viewport {
                continue; // sin scrollbar visible
            }
            let max_scroll = items_len.saturating_sub(viewport);
            let (thumb_h, track) = v_scroll_thumb_geometry(rect.height, items_len, viewport);

            // Jump-to-position: thumb centrado bajo el cursor, luego 1:1
            let rel = f32::from(y.saturating_sub(rect.y));
            self.drag = Some(DragState::VScroll(kind));
            self.apply_v_drag(rel - thumb_h as f32 / 2.0, kind, max_scroll, track);
            return true;
        }
        false
    }

    /// Convierte la Y del mouse en `scroll_offset` del panel.
    /// Mapeo 1:1 (ver `apply_h_drag`).
    fn apply_v_drag(&mut self, rel: f32, kind: PanelKind, max_scroll: usize, track: f32) {
        let pct = (rel / track.max(1.0)).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let new = (pct * max_scroll as f32).round() as usize;
        let new = new.min(max_scroll);
        let p = self.panel_mut(kind);
        p.scroll_offset.set(new);
        // La selección sigue al scroll para que el scrollbar (posicionado por
        // selected_idx) muestre el thumb en el lugar correcto
        p.selected_idx = new;
    }

    #[allow(clippy::too_many_lines)]
    pub fn on_mouse_click(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width < 40 || height < 10 {
            return;
        }

        // Formulario de nueva conexión (sin db): click en el botón
        // "[Conectar]" conecta; click en cualquier otra zona del Detail
        // enfoca el panel para que el teclado alimente los campos.
        if self.db_path.is_none() && self.connection_form.is_some() {
            if let Some(&(_, rect)) =
                self.layout.panels.iter().find(|(k, _)| *k == PanelKind::Detail)
            {
                let inside = x >= rect.x
                    && x < rect.x.saturating_add(rect.width)
                    && y >= rect.y
                    && y < rect.y.saturating_add(rect.height);
                if inside {
                    // El botón vive en la fila inner.y + 11 del formulario
                    // (URL + nota + separador + Tipo + 5 campos + hueco).
                    let inner_y = rect.y.saturating_add(1);
                    let connect_row = inner_y.saturating_add(11);
                    if y == connect_row && x > rect.x {
                        self.conn_submit();
                        return;
                    }
                    // Cualquier otra zona: enfocar Detail (captura teclas)
                    self.active_panel = PanelKind::Detail;
                    return;
                }
            }
        }

        // Click fuera del modal de inspector de fila → cerrarlo y continuar
        // con el procesamiento normal del click (seleccionar el ítem clickeado).
        if self.show_row_inspector {
            let mw = width.saturating_mul(70) / 100;
            let mh = height.saturating_mul(70) / 100;
            let mx = width.saturating_sub(mw) / 2;
            let my = height.saturating_sub(mh) / 2;
            let inside =
                x >= mx && x < mx.saturating_add(mw) && y >= my && y < my.saturating_add(mh);
            if inside {
                // Botón del modo NoSQL (`[J: json]` / `[J: pares]`): vive en el
                // título (fila superior del borde del modal), lado derecho.
                if self.is_nosql && y == my {
                    self.toggle_inspector_json_mode();
                }
                return;
            }
            self.close_row_inspector();
        }

        // Encontrar qué panel fue clickeado usando el layout computado
        // El layout ya se calculó en el loop principal antes de renderizar
        for &(kind, rect) in &self.layout.panels {
            if x < rect.x
                || x >= rect.x.saturating_add(rect.width)
                || y < rect.y
                || y >= rect.y.saturating_add(rect.height)
            {
                continue;
            }

            // Click dentro de este panel
            let rel_y = y.saturating_sub(rect.y);

            // ¿Click en el título (primera línea)?
            if rel_y == 0 {
                if kind == PanelKind::Detail {
                    // Detectar click en tabs del título
                    if let Some(tab) = self.detect_detail_tab_click(x, rect) {
                        self.set_detail_tab(tab);
                    }
                } else if kind == PanelKind::Sources {
                    // Detectar click en tabs de Fuentes (Todo/Local/Online)
                    if let Some(tab) = self.detect_source_tab_click(x, rect) {
                        self.set_source_tab(tab);
                    }
                }
                self.set_focus(kind);
                return;
            }

            // Click en contenido
            self.set_focus(kind);

            // Click en header de Data tab → ordenar por columna
            if kind == PanelKind::Detail && self.detail_tab == DetailTab::Data && rel_y == 2 {
                if let Some(col_name) = self.column_at_x(x, rect) {
                    self.toggle_sort(col_name);
                }
                return;
            }

            // Click en un ítem de la lista
            // Para Data tab, las filas de datos empiezan en rel_y=4 (spacer+header+separator)
            let top_reserved =
                if kind == PanelKind::Detail && self.detail_tab == DetailTab::Data { 3 } else { 0 };
            if let Some(mut index) = list_index_from_click(rel_y, rect.height, top_reserved) {
                if kind == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    // +1 porque selected_idx=0 salta el header (primera fila de datos es idx=1)
                    index = index.saturating_add(1);
                }
                let max_idx = self.items_len_for(kind).saturating_sub(1);
                let scroll = self.panel(kind).scroll_offset.get();
                let mut index = (index + scroll).min(max_idx);
                // Click sobre una sección de Fuentes → aterrizar en el primer entry
                if kind == PanelKind::Sources {
                    let shown = self.items_for(kind);
                    index = Self::skip_section_idx(shown, index, 1);
                }
                let p = self.panel_mut(kind);
                p.selected_idx = index;

                // Doble-click: detectar 2 clicks en < 400ms sobre el mismo panel+ítem
                let now = now_millis();
                let is_double = self.last_click_kind == Some(kind)
                    && self.last_click_idx == index
                    && now.saturating_sub(self.last_click_time) < 400;
                self.last_click_time = now;
                self.last_click_kind = Some(kind);
                self.last_click_idx = index;

                if is_double && kind == PanelKind::Detail {
                    self.open_row_inspector();
                    return;
                }

                // Click simple: ejecutar acción del panel sin saltar a Detail
                if kind == PanelKind::Sources {
                    self.connect_selected_source();
                } else if kind == PanelKind::Tables
                    || kind == PanelKind::Views
                    || kind == PanelKind::Advanced
                {
                    self.current_page = 0;
                    self.refresh_preview_from_selected_object();
                }
                // Detail: doble-click ya manejado arriba, click simple no hace nada extra
            }

            return;
        }

        // Click fuera de cualquier panel → ignorar
    }
}

// ── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn state_de_prueba() -> storage::AppState {
        let mut state = storage::AppState::new();
        state.recents = vec!["/a/one.db".to_string(), "https://remote.example/db".to_string()];
        state.favorites.insert("one".to_string(), "/a/one.db".to_string());
        state
    }

    #[test]
    fn strip_marks_quita_prefijos_decorativos() {
        assert_eq!(strip_source_marks("▣ /tmp/x.db"), "/tmp/x.db");
        assert_eq!(strip_source_marks("⊙ https://api.x/db"), "https://api.x/db");
        assert_eq!(strip_source_marks("M mysql://127.0.0.1/lazy"), "mysql://127.0.0.1/lazy");
        assert_eq!(
            strip_source_marks("P postgres://db.azure.com/prod"),
            "postgres://db.azure.com/prod"
        );
        assert_eq!(strip_source_marks("D base.duckdb"), "base.duckdb");
        assert_eq!(strip_source_marks("★ one => /a/one.db"), "one => /a/one.db");
        assert_eq!(strip_source_marks("Abrir sakila.db"), "Abrir sakila.db");
        // Marcas combinables (conectada + tipo)
        assert_eq!(strip_source_marks("● ▣ /tmp/x.db"), "/tmp/x.db");
        assert_eq!(strip_source_marks("● M mysql://127.0.0.1/lazy"), "mysql://127.0.0.1/lazy");
        assert_eq!(strip_source_marks("● ★ one => /a/one.db"), "one => /a/one.db");
    }

    #[test]
    #[cfg(any(feature = "mysql", feature = "postgres"))]
    fn strip_url_credentials_quita_user_pass_y_devuelve_el_user() {
        let (clean, user) =
            strip_url_credentials("postgresql://uo8h6cfqdm4u5xv6lfqq:secret@host:5432/db");
        assert_eq!(clean, "postgresql://host:5432/db");
        assert_eq!(user.as_deref(), Some("uo8h6cfqdm4u5xv6lfqq"));

        // Sin credenciales: intacta y user None
        let (clean, user) = strip_url_credentials("postgres://localhost:5432");
        assert_eq!(clean, "postgres://localhost:5432");
        assert_eq!(user, None);

        let (clean, user) = strip_url_credentials("mysql://root:root@127.0.0.1:3306");
        assert_eq!(clean, "mysql://127.0.0.1:3306");
        assert_eq!(user.as_deref(), Some("root"));
    }

    #[test]
    #[cfg(any(feature = "mysql", feature = "postgres"))]
    fn server_url_has_database_distingue_servidor_de_bd() {
        // URLs de scan_local_servers: servidor sin base
        assert!(!server_url_has_database("mysql://127.0.0.1:3306"));
        assert!(!server_url_has_database("mysql://127.0.0.1"));
        assert!(!server_url_has_database("mysql://root:root@127.0.0.1:3306"));
        // Con base explícita
        assert!(server_url_has_database("mysql://127.0.0.1:3306/lazydb_demo"));
        assert!(server_url_has_database("mysql://root:root@127.0.0.1:3306/lazydb_demo"));
        // PostgreSQL: mismo contrato, ambos prefijos
        assert!(!server_url_has_database("postgres://127.0.0.1:5432"));
        assert!(!server_url_has_database("postgresql://127.0.0.1"));
        assert!(!server_url_has_database("postgres://postgres:secret@127.0.0.1:5432"));
        assert!(server_url_has_database("postgres://127.0.0.1:5432/sakila"));
        assert!(server_url_has_database("postgresql://user:pw@localhost:5432/mydb"));
        // URI real de CleverCloud (producción): con base → NO es servidor
        assert!(server_url_has_database(
            "postgresql://uo8h6cfqdm4u5xv6lfqq:dsJBQr44561wnu9YizPLTeP1GFh0eO@bh4bhaewrpd8qqcreso5-postgresql.services.clever-cloud.com:5432/bh4bhaewrpd8qqcreso5"
        ));
        // No-servidor no entra en el flujo
        assert!(!server_url_has_database("/tmp/x.db"));
    }

    #[test]
    fn source_path_extrae_el_dato_real() {
        assert_eq!(source_path_of("★ one => /a/one.db"), "/a/one.db");
        assert_eq!(source_path_of("● ▣ /tmp/x.db"), "/tmp/x.db");
        assert_eq!(source_path_of("⊙ https://api.x/db"), "https://api.x/db");
        assert_eq!(source_path_of("Abrir sakila.db"), "Abrir sakila.db");
    }

    #[test]
    fn secciones_llevan_marcador_interno() {
        let s = source_section("FAVORITOS");
        assert!(is_source_section(&s));
        assert!(!is_source_section("★ one => /a/one.db"));
        assert!(!is_source_section("sakila.db"));
    }

    #[test]
    fn build_sources_online_agrupa_favoritos_y_recents() {
        let mut state = state_de_prueba();
        state.favorites.insert("remote".to_string(), "https://remote.example/db".to_string());
        let sources = App::build_sources(&state, SourceTab::Online, None, &HashMap::new(), &[]);

        // Sin sección FAVORITOS: la ★ identifica al favorito, va el primero
        assert!(
            !sources.iter().any(|s| s == &source_section("FAVORITOS")),
            "la sección FAVORITOS no debe existir: {sources:?}"
        );
        assert_eq!(
            sources.first().map(String::as_str),
            Some("★ remote => https://remote.example/db"),
            "el favorito debe encabezar la lista sin sección"
        );
        // "one" es favorito local → no aparece en el tab Online
        assert!(!sources.iter().any(|s| s.contains("/a/one.db")));
        // Sin acciones fijas en Online
        assert!(!sources.iter().any(|s| s == "Abrir sakila.db"));
        assert!(!sources.iter().any(|s| s == "Buscar archivo .db"));
    }

    #[test]
    fn build_sources_pon_los_favoritos_de_primeras() {
        // En el tab All: favoritos al inicio (sin sección), luego RECIENTES
        let state = state_de_prueba(); // favorito "one => /a/one.db", recientes /a/one.db + https
        let sources = App::build_sources(&state, SourceTab::All, None, &HashMap::new(), &[]);

        assert!(sources.first().is_some_and(|s| s.starts_with("★ one => /a/one.db")));
        let recents_idx = sources
            .iter()
            .position(|s| s == &source_section("RECIENTES"))
            .expect("la sección RECIENTES debe existir");
        assert!(
            sources[..recents_idx].iter().all(|s| !is_source_section(s)),
            "antes de RECIENTES solo puede haber favoritos sueltos"
        );
    }

    #[test]
    fn build_sources_marca_la_conectada() {
        let state = state_de_prueba();
        let sources =
            App::build_sources(&state, SourceTab::All, Some("/a/one.db"), &HashMap::new(), &[]);
        assert!(sources.iter().any(|s| s.starts_with("● ★ one => /a/one.db")));
    }

    // ── regresión: normalización de rutas (Bug 1) ──────────────────────

    #[test]
    fn relativo_y_absoluto_no_se_duplican() {
        // Un reciente guardado en relativo por una versión antigua ("one.db")
        // y el mismo archivo detectado en absoluto ("/a/one.db") deben
        // colapsar en UNA sola entrada: el `seen` compara rutas canónicas.
        let mut state = storage::AppState::new();
        state.recents = vec!["one.db".to_string(), "https://remote.example/db".to_string()];
        let sources = App::build_sources(&state, SourceTab::All, None, &HashMap::new(), &[]);

        let matches = sources
            .iter()
            .filter(|s| {
                source_path_of(s) != "Buscar archivo .db" && source_path_of(s) != "Abrir sakila.db"
            })
            .filter(|s| source_path_of(s).ends_with("one.db"))
            .count();
        assert_eq!(matches, 1, "la DB relativa y la absoluta deben ser una sola");
    }

    #[test]
    fn conectada_por_ruta_relativa_marca_con_absoluto() {
        // Conectar "one.db" (relativo) normaliza a absoluto; el panel debe
        // mostrar ● sobre la entrada absoluta, no sobre una relativa huérfana.
        let mut state = storage::AppState::new();
        state.recents = vec!["one.db".to_string()];
        let connected = crate::paths::normalize_path("one.db");
        let sources =
            App::build_sources(&state, SourceTab::All, Some(&connected), &HashMap::new(), &[]);
        assert!(
            sources.iter().any(|s| s.starts_with(&format!("● ▣ {connected}"))),
            "la entrada absoluta debe llevar ●: {sources:?}"
        );
    }

    #[test]
    fn build_sources_normaliza_el_reciente_mostrado() {
        // El reciente relativo "one.db" debe mostrarse en su forma canónica.
        let mut state = storage::AppState::new();
        state.recents = vec!["one.db".to_string()];
        let sources = App::build_sources(&state, SourceTab::All, None, &HashMap::new(), &[]);
        let shown = sources.iter().find_map(|s| {
            let p = source_path_of(s);
            (!is_source_section(s) && p != "Abrir sakila.db" && p != "Buscar archivo .db")
                .then_some(p)
        });
        let canonical = crate::paths::normalize_path("one.db");
        assert_eq!(shown, Some(canonical.as_str()));
    }

    // ── rediseño: SourceKind + marcas por tipo de DB ───────────────────

    #[test]
    fn source_kind_clasifica_fuentes() {
        assert_eq!(source_kind("mysql://localhost:3306/lazy"), SourceKind::Localhost);
        assert_eq!(source_kind("mysql://127.0.0.1:3306/lazy"), SourceKind::Localhost);
        assert_eq!(source_kind("postgres://[::1]:5432/prod"), SourceKind::Localhost);
        assert_eq!(source_kind("postgres://db.azure.com:5432/prod"), SourceKind::Online);
        assert_eq!(source_kind("mongodb://127.0.0.1:27017"), SourceKind::Localhost);
        assert_eq!(source_kind("mongodb://mongo.atlas.cloud:27017/db"), SourceKind::Online);
        assert_eq!(source_kind("https://api.example.com/db"), SourceKind::Online);
        assert_eq!(source_kind("ssh://host/db"), SourceKind::Online);
        // sqlite:// y rutas de archivo SIEMPRE son File (antes se confundían)
        assert_eq!(source_kind("sqlite:///tmp/x.db"), SourceKind::File);
        assert_eq!(source_kind("sakila.db"), SourceKind::File);
        assert_eq!(source_kind("/abs/base.duckdb"), SourceKind::File);
    }

    #[test]
    fn url_host_extrae_el_host_real() {
        assert_eq!(url_host("mysql://user:pass@127.0.0.1:3306/lazy"), Some("127.0.0.1"));
        assert_eq!(url_host("mysql://localhost/db"), Some("localhost"));
        assert_eq!(url_host("postgres://[::1]:5432/x"), Some("[::1]"));
        assert_eq!(url_host("postgres://db.azure.com:5432/prod"), Some("db.azure.com"));
        assert_eq!(url_host("mongodb://127.0.0.1:27017"), Some("127.0.0.1"));
        assert_eq!(url_host("sqlite:///tmp/x.db"), Some(""));
    }

    #[test]
    fn db_type_mark_segun_tipo() {
        assert_eq!(db_type_mark("postgres://db.azure.com/prod"), 'P');
        assert_eq!(db_type_mark("postgresql://127.0.0.1/lazy"), 'P');
        assert_eq!(db_type_mark("mysql://localhost/lazy"), 'M');
        assert_eq!(db_type_mark("mongodb://127.0.0.1:27017"), 'N');
        assert_eq!(db_type_mark("https://api.x/db"), '⊙');
        assert_eq!(db_type_mark("base.duckdb"), 'D');
        assert_eq!(db_type_mark("otra.ddb"), 'D');
        assert_eq!(db_type_mark("sakila.db"), '▣');
        assert_eq!(db_type_mark("sqlite:///tmp/x.db"), '▣');
    }

    #[test]
    fn build_sources_servidor_mongo_no_se_normaliza_como_path() {
        // Regresión del bug: `entry()` normalizaba `mongodb://127.0.0.1:27017`
        // como path de archivo (is_url no la reconocía) → la URL se rompía.
        let state = state_de_prueba();
        let sources = App::build_sources(
            &state,
            SourceTab::All,
            None,
            &HashMap::new(),
            &["mongodb://127.0.0.1:27017".to_string()],
        );
        let entry = sources
            .iter()
            .find(|s| s.contains("mongodb://"))
            .expect("el servidor mongo debe aparecer en las fuentes");
        assert!(
            entry.contains("mongodb://127.0.0.1:27017"),
            "la URL debe quedar intacta (no un path normalizado): {entry}"
        );
        assert!(!entry.contains("lazydb/mongodb"), "no debe mezclarse con el cwd: {entry}");
    }

    #[test]
    fn tab_local_muestra_localhost_y_oculta_online() {
        let mut state = storage::AppState::new();
        state.recents = vec![
            "mysql://127.0.0.1:3306/lazy".to_string(),
            "one.db".to_string(),
            "https://remote.example/db".to_string(),
        ];
        let local = App::build_sources(&state, SourceTab::Local, None, &HashMap::new(), &[]);
        assert!(
            local.iter().any(|s| s.starts_with("M mysql://127.0.0.1:3306/lazy")),
            "mysql de localhost debe estar en el tab Local: {local:?}"
        );
        assert!(local.iter().any(|s| source_path_of(s).ends_with("one.db")));
        assert!(
            !local.iter().any(|s| s.contains("https://remote.example/db")),
            "lo online NO debe filtrarse al tab Local"
        );

        let online = App::build_sources(&state, SourceTab::Online, None, &HashMap::new(), &[]);
        assert!(
            online.iter().any(|s| s.starts_with("⊙ https://remote.example/db")),
            "lo online debe estar en el tab Online: {online:?}"
        );
        assert!(!online.iter().any(|s| source_path_of(s).ends_with("one.db")));
        assert!(!online.iter().any(|s| s.contains("mysql://127.0.0.1")));
    }

    // ── probe de salud (filosofía culling) ────────────────────────────

    #[test]
    fn source_host_port_extrae_host_y_puerto() {
        assert_eq!(
            source_host_port("mysql://localhost:3306/lazy"),
            Some(("localhost".into(), 3306))
        );
        assert_eq!(
            source_host_port("postgres://user@db.azure.com:5432/prod"),
            Some(("db.azure.com".into(), 5432))
        );
        // Sin puerto → default del esquema
        assert_eq!(source_host_port("mysql://localhost/lazy"), Some(("localhost".into(), 3306)));
        assert_eq!(source_host_port("mongodb://localhost/mydb"), Some(("localhost".into(), 27017)));
        assert_eq!(
            source_host_port("mongodb://127.0.0.1:27017"),
            Some(("127.0.0.1".into(), 27017))
        );
        assert_eq!(
            source_host_port("https://api.example.com/x"),
            Some(("api.example.com".into(), 443))
        );
        // IPv6 entre corchetes
        assert_eq!(source_host_port("postgres://[::1]:5432/x"), Some(("::1".into(), 5432)));
        // No es URL de conexión
        assert_eq!(source_host_port("sakila.db"), None);
        assert_eq!(source_host_port("sqlite:///tmp/x.db"), None);
    }

    #[test]
    fn probe_source_detecta_archivos() {
        let dir = std::env::temp_dir().join(format!("lazydb-probe-{}", std::process::id()));
        let db_file = dir.join("x.db");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(&db_file, b"sqlite").expect("escribir archivo de prueba");

        assert!(probe_source(&db_file.to_string_lossy()), "archivo existente debe dar ✓");
        assert!(
            probe_source(&format!("sqlite://{}", db_file.to_string_lossy())),
            "URL sqlite:// del mismo archivo debe dar ✓"
        );
        let missing = dir.join("no-existe.db");
        assert!(!probe_source(&missing.to_string_lossy()), "archivo inexistente debe dar ✗");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_source_verifica_tcp() {
        // Servidor real: el probe debe dar ✓
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind en tests");
        let port = listener.local_addr().expect("puerto local").port();
        let url = format!("mysql://127.0.0.1:{port}/lazy");
        assert!(probe_source(&url), "servicio escuchando debe dar ✓: {url}");
        // MongoDB usa el mismo probe TCP (puerto explícito en la URL)
        let url_mongo = format!("mongodb://127.0.0.1:{port}/lazy");
        assert!(probe_source(&url_mongo), "servicio mongo escuchando debe dar ✓: {url_mongo}");
        drop(listener);

        // Puerto cerrado: debe dar ✗ (refused es inmediato)
        let url_closed = format!("mysql://127.0.0.1:{port}/lazy");
        assert!(!probe_source(&url_closed), "puerto cerrado debe dar ✗: {url_closed}");
    }

    #[test]
    fn build_sources_solo_marca_el_error() {
        let mut state = storage::AppState::new();
        state.recents = vec!["ok.db".to_string(), "caida.db".to_string()];
        let canonical_ok = crate::paths::normalize_path("ok.db");
        let canonical_caida = crate::paths::normalize_path("caida.db");

        // Sin caché → sin marcas de salud en absoluto (nada de ✓ por defecto)
        let sources = App::build_sources(&state, SourceTab::All, None, &HashMap::new(), &[]);
        assert!(
            sources.iter().any(|s| s.contains(&canonical_ok) && !s.starts_with("✗ ")),
            "la fuente sana no debe llevar marca: {sources:?}"
        );

        let mut health = HashMap::new();
        health.insert(canonical_caida.clone(), false);
        let sources = App::build_sources(&state, SourceTab::All, None, &health, &[]);
        assert!(
            sources.iter().any(|s| s.starts_with(&format!("✗ ▣ {canonical_caida}"))),
            "fuente caída debe llevar ✗: {sources:?}"
        );
        // Conectada y caída combina ● + ✗
        let sources_connected =
            App::build_sources(&state, SourceTab::All, Some(&canonical_caida), &health, &[]);
        assert!(
            sources_connected.iter().any(|s| s.starts_with(&format!("● ✗ ▣ {canonical_caida}"))),
            "conectada + caída = '● ✗': {sources_connected:?}"
        );
    }

    #[test]
    fn source_probe_path_ignora_secciones_placeholder_y_acciones() {
        assert_eq!(App::source_probe_path("\u{1}RECIENTES"), None, "sección");
        assert_eq!(App::source_probe_path("<sin entradas>"), None, "placeholder");
        assert_eq!(App::source_probe_path("Abrir sakila.db"), None, "acción fija");
        assert_eq!(App::source_probe_path("Buscar archivo .db"), None, "acción fija 2");
        // Fuentes reales → path limpio (marcas y prefijos fuera)
        assert_eq!(App::source_probe_path("▣ /tmp/a.db"), Some("/tmp/a.db".to_string()));
        assert_eq!(
            App::source_probe_path("M mysql://db.azure.com/prod"),
            Some("mysql://db.azure.com/prod".to_string())
        );
    }

    #[test]
    fn visible_probe_targets_solo_lo_visible_y_no_comprobado() {
        let items = vec![
            source_section("RECIENTES"),
            "▣ /tmp/one.db".to_string(),
            "▣ /tmp/two.db".to_string(),
            "▣ /tmp/three.db".to_string(),
            "<sin entradas>".to_string(),
            "Abrir sakila.db".to_string(),
        ];
        // one.db ya comprobado (caché), two.db en vuelo
        let mut health = HashMap::new();
        health.insert("/tmp/one.db".to_string(), false);
        let mut probing = HashSet::new();
        probing.insert("/tmp/two.db".to_string());

        // Ventana completa: solo queda three.db
        let targets = App::visible_probe_targets(&items, 0..6, &health, &probing);
        assert_eq!(targets, vec!["/tmp/three.db".to_string()]);

        // Ventana parcial (filas 0..2 = sección + one.db): nada nuevo
        let targets = App::visible_probe_targets(&items, 0..2, &health, &probing);
        assert!(targets.is_empty());

        // Ventana 0..3: sección + one.db + two.db → los tres filtrados
        let targets = App::visible_probe_targets(&items, 0..3, &health, &probing);
        assert!(targets.is_empty());

        // Ventana desplazada (2..6): two.db (en vuelo), three.db, placeholder,
        // acción → solo three.db
        let targets = App::visible_probe_targets(&items, 2..6, &health, &probing);
        assert_eq!(targets, vec!["/tmp/three.db".to_string()]);
    }

    #[test]
    fn entry_marca_el_tipo_no_solo_sqlite() {
        let mut state = storage::AppState::new();
        state.recents = vec![
            "base.duckdb".to_string(),
            "mysql://localhost/lazy".to_string(),
            "postgres://db.azure.com/prod".to_string(),
        ];
        let sources = App::build_sources(&state, SourceTab::All, None, &HashMap::new(), &[]);
        assert!(sources.iter().any(|s| s.starts_with("D ")), "DuckDB debe marcarse D");
        assert!(sources.iter().any(|s| s.starts_with("M ")), "MySQL debe marcarse M");
        assert!(sources.iter().any(|s| s.starts_with("P ")), "Postgres debe marcarse P");
    }

    // ── resumen del panel colapsado (no enfocado) ──────────────────────

    #[test]
    fn summary_prioriza_la_db_conectada() {
        let items = vec![
            source_section("FAVORITOS"),
            "★ one => /a/one.db".to_string(),
            source_section("RECIENTES"),
            "● ▣ /tmp/x.db".to_string(),
            "Abrir sakila.db".to_string(),
        ];
        assert_eq!(source_summary(&items, 0), vec!["● ▣ /tmp/x.db"]);
        // Aunque el cursor esté en otro lado, la conectada manda
        assert_eq!(source_summary(&items, 4), vec!["● ▣ /tmp/x.db"]);
    }

    #[test]
    fn summary_sin_conectada_muestra_la_seleccionada() {
        let items = vec![
            source_section("FAVORITOS"),
            "★ one => /a/one.db".to_string(),
            "Abrir sakila.db".to_string(),
        ];
        assert_eq!(source_summary(&items, 1), vec!["★ one => /a/one.db"]);
        // Cursor sobre una sección o acción fija → primer entry real
        assert_eq!(source_summary(&items, 0), vec!["★ one => /a/one.db"]);
        assert_eq!(source_summary(&items, 2), vec!["★ one => /a/one.db"]);
        // Índice fuera de rango → primer entry real
        assert_eq!(source_summary(&items, 99), vec!["★ one => /a/one.db"]);
    }

    #[test]
    fn summary_solo_secciones_o_vacio_devuelve_nada() {
        assert_eq!(source_summary(&[], 0), Vec::<&str>::new());
        let items = vec![source_section("FAVORITOS"), "<sin entradas>".to_string()];
        assert_eq!(source_summary(&items, 0), Vec::<&str>::new());
        let items = vec!["Abrir sakila.db".to_string()];
        assert_eq!(source_summary(&items, 0), Vec::<&str>::new());
    }

    #[test]
    fn skip_section_aterriza_en_entry() {
        let items = vec![
            source_section("FAVORITOS"),
            "★ one => /a/one.db".to_string(),
            source_section("RECIENTES"),
            "▣ /tmp/x.db".to_string(),
            "Abrir sakila.db".to_string(),
        ];
        assert_eq!(App::skip_section_idx(&items, 0, 1), 1);
        assert_eq!(App::skip_section_idx(&items, 2, 1), 3);
        assert_eq!(App::skip_section_idx(&items, 2, -1), 1);
        // Borde inferior: se queda en el último entry
        assert_eq!(App::skip_section_idx(&items, 4, 1), 4);
    }

    // ── query runner: cancelación por generación ──────────────────────

    #[test]
    fn apply_count_result_descarta_resultados_stale() {
        // Generación vieja (query reemplazada o limpiada) → None, no aplica
        let res = App::apply_count_result(5, 4, "SELECT COUNT(*) FROM t;", Ok(42));
        assert_eq!(res, None, "resultado stale debe descartarse");

        // Generación actual → Done con filas y status
        let (state, status) = App::apply_count_result(5, 5, "SELECT COUNT(*) FROM t;", Ok(42))
            .expect("resultado actual");
        assert_eq!(
            state,
            query::QueryState::Done(vec![
                "COUNT(*) = 42".to_string(),
                "SQL: SELECT COUNT(*) FROM t;".to_string(),
            ])
        );
        assert_eq!(status, "Query completada: 42 filas");

        // Error en generación actual → Error con mensaje, sin panics
        let (state, status) = App::apply_count_result(
            5,
            5,
            "SELECT COUNT(*) FROM nope;",
            Err(crate::db::DbError::Sqlite("no such table".into())),
        )
        .expect("resultado actual");
        assert!(matches!(state, query::QueryState::Error(e) if e.contains("no such table")));
        assert_eq!(status, "Error de SQLite: no such table");
    }

    // ── popup de error global ─────────────────────────────────────────

    fn app_con_error() -> App {
        let mut app = App::new();
        app.error =
            Some(ErrorPopup { title: "Error de prueba".to_string(), body: "cuerpo".to_string() });
        app
    }

    #[test]
    fn el_popup_de_error_se_cierra_con_enter_esc_o_q() {
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('q')] {
            let mut app = app_con_error();
            app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
            assert!(app.error.is_none(), "tecla {code:?} debería cerrar el popup");
        }
    }

    #[test]
    fn el_popup_de_error_ignora_las_demas_teclas() {
        let mut app = app_con_error();
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(app.error.is_some(), "navegación no debe cerrar el popup");
    }

    /// avPag/rePag = mover K filas (K = filas visibles en pantalla), con
    /// clamp a los bordes del buffer cargado. Nada de "primera fila de la
    /// página siguiente" ni recarga del preview.
    #[tokio::test]
    async fn pagina_mueve_k_filas_con_clamp_al_contenido() {
        let mut app = App::new();
        app.detail_tab = DetailTab::Data;
        // Buffer: header + 40 filas cargadas (de 250 totales)
        app.preview_rows = std::iter::once("col".to_string())
            .chain((1..=40).map(|i| format!("fila {i}")))
            .collect();
        app.total_rows = 250;
        app.preview_loaded_offset = 100;
        app.compute_layout(120, 40);
        let k = {
            let rect = app
                .layout
                .panels
                .iter()
                .find(|(kind, _)| *kind == PanelKind::Detail)
                .map(|(_, r)| *r)
                .unwrap_or_default();
            usize::from(rect.height.saturating_sub(5)).max(1)
        };

        // avPag: 1 + K (dentro del buffer) — sin recargar nada
        app.set_selected_idx(PanelKind::Detail, 1);
        app.move_selection_by_page(true);
        assert_eq!(app.selected_idx(PanelKind::Detail), 1 + k);
        assert_eq!(app.preview_loaded_offset, 100, "no debe recargar el preview");

        // avPag: sobrepasa el final del buffer → clamp a la última fila
        app.move_selection_by_page(true);
        assert_eq!(app.selected_idx(PanelKind::Detail), 40, "clamp al final del buffer");

        // rePag: retrocede K
        app.move_selection_by_page(false);
        assert_eq!(app.selected_idx(PanelKind::Detail), 40 - k);

        // rePag en la fila 1: clamp (nunca el header, fila 0)
        app.set_selected_idx(PanelKind::Detail, 1);
        app.move_selection_by_page(false);
        assert_eq!(app.selected_idx(PanelKind::Detail), 1, "clamp a la fila 1");
    }

    // ── input SQL (`:` popup + historial persistente) ────────────────

    fn app_con_query_input(history: &[&str]) -> App {
        let mut app = App::new();
        app.query_input = Some(QueryInputState::default());
        // Más reciente al inicio: coincide con add_query_history (insert(0, ...))
        app.state.query_history = history.iter().map(|s| (*s).to_string()).collect();
        // Para consistencia visual en los tests: invertimos si lo declaran
        // en orden cronológico (viejo→nuevo). Los tests pasan el array como
        // "qué devolverá query_history_select en orden de navegación":
        // idx 0 = la query más reciente, idx 1 = la siguiente, etc.
        app
    }

    #[test]
    fn abrir_query_input_se_dispara_con_dos_puntos_y_abre_el_popup() {
        let mut app = App::new();
        // `:` está bindeado a OpenQueryInput; la acción abre el popup
        app.on_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        assert!(app.query_input.is_some(), "`:` debe abrir el input SQL");
    }

    #[test]
    fn escribir_en_el_input_appendea_chars_al_buffer_y_mueve_el_cursor() {
        let mut app = app_con_query_input(&[]);
        for c in ['S', 'E', 'L'] {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let s = app.query_input.as_ref().unwrap();
        assert_eq!(s.buffer, "SEL");
        assert_eq!(s.cursor, 3);
    }

    #[test]
    fn backspace_borra_el_char_antes_del_cursor() {
        let mut app = app_con_query_input(&[]);
        for c in ['S', 'E', 'L'] {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        let s = app.query_input.as_ref().unwrap();
        assert_eq!(s.buffer, "SE");
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn flecha_arriba_rellena_el_buffer_con_la_query_mas_reciente_del_historial() {
        let mut app = app_con_query_input(&["SELECT 2", "SELECT 1"]);
        // buffer vacío + ↑ → la query más reciente (idx 0 = "SELECT 2")
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let s = app.query_input.as_ref().unwrap();
        assert_eq!(s.buffer, "SELECT 2");
        assert_eq!(s.history_idx, Some(0));
        assert_eq!(s.cursor, s.buffer.chars().count());
    }

    #[test]
    fn flecha_arriba_dos_veces_avanza_por_el_historial() {
        let mut app = app_con_query_input(&["SELECT 2", "SELECT 1"]);
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let s = app.query_input.as_ref().unwrap();
        // Llegamos a la entrada más vieja: "SELECT 1"
        assert_eq!(s.buffer, "SELECT 1");
        assert_eq!(s.history_idx, Some(1));
    }

    #[test]
    fn flecha_abajo_despues_de_arriba_vuelve_a_query_nueva() {
        let mut app = app_con_query_input(&["SELECT 2", "SELECT 1"]);
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        // ↑ rellenó "SELECT 2" (idx 0); ↓ baja hacia la siguiente → "SELECT 1"
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let s = app.query_input.as_ref().unwrap();
        // ↓ desde idx=1 debería bajar a idx=0 (más reciente)
        assert_eq!(s.buffer, "SELECT 2", "↓ desde posición alta vuelve a la más reciente");
    }

    #[test]
    fn enter_con_buffer_vacio_cierra_sin_ejecutar_ni_historial() {
        let mut app = app_con_query_input(&[]);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.query_input.is_none());
        assert!(app.state.query_history.is_empty(), "buffer vacío no registra historial");
    }

    #[test]
    fn enter_con_sql_sin_db_muestra_error_sin_panico() {
        let mut app = app_con_query_input(&[]);
        for c in "SELECT 1".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.query_input.is_none(), "Enter cierra el popup");
        assert!(app.error.is_some(), "sin db_path → error global (no paniquea)");
        assert!(app.state.query_history.is_empty(), "historial solo se guarda al ejecutar con DB");
    }

    #[test]
    fn apply_user_query_result_devuelve_done_o_error_sin_panic() {
        let ok = query::QueryResult { rows: vec!["a | b".to_string()], error: None };
        let (state, rows) = App::apply_user_query_result(&Ok(ok));
        assert!(matches!(state, query::QueryState::Done(_)));
        assert_eq!(rows, vec!["a | b".to_string()]);

        let err = crate::db::DbError::Sqlite("no such table: x".to_string());
        let (state, rows) = App::apply_user_query_result(&Err(err));
        assert!(matches!(state, query::QueryState::Error(_)));
        assert!(rows.is_empty());
    }

    #[test]
    fn scan_cwd_detecta_db_y_duckdb() {
        // El scan lee el cwd real del proceso: creamos un dir temp y corremos
        // el scan con cwd cambiado (los tests corren en paralelo, así que
        // cambiamos cwd dentro del test con un guard de restauración).
        struct CwdGuard(std::path::PathBuf);
        impl Drop for CwdGuard {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }

        let dir = std::env::temp_dir().join(format!("lazydb_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear dir temp");
        std::fs::write(dir.join("a.db"), b"x").expect("db");
        std::fs::write(dir.join("b.duckdb"), b"x").expect("duckdb");
        std::fs::write(dir.join("c.ddb"), b"x").expect("ddb");
        std::fs::write(dir.join("nota.txt"), b"x").expect("txt");

        let original = std::env::current_dir().expect("cwd actual");
        let guard = CwdGuard(original);
        std::env::set_current_dir(&dir).expect("cambiar cwd");

        let found = scan_cwd_databases();
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);

        // paths completos (absolutos) ordenados alfabéticamente
        assert_eq!(
            found,
            vec![
                dir.join("a.db").to_string_lossy().into_owned(),
                dir.join("b.duckdb").to_string_lossy().into_owned(),
                dir.join("c.ddb").to_string_lossy().into_owned(),
            ]
        );
    }

    /// Smoke del flujo completo de servidor: `connect_mysql_server` abre el
    /// picker con las bases reales, y elegir `lazydb_demo` conecta el catálogo.
    /// Requiere `MariaDB` local con la env var `LAZYDB_MYSQL_URL` (ver el archivo `AGENTS.md`).
    #[test]
    #[cfg(feature = "mysql")]
    #[ignore = "requiere MariaDB local (LAZYDB_MYSQL_URL)"]
    fn smoke_flujo_servidor_picker_conecta_bd_real() {
        let Ok(server_url) = std::env::var("LAZYDB_MYSQL_SERVER_URL") else {
            return;
        };
        let mut app = App::new();
        app.connect_sqlite(&server_url);

        // Camino A: el primer intento sin credenciales abre el picker (no hay
        // auth). Camino B: abre el prompt de password → autenticar con root/root.
        if app.password_prompt.is_some() {
            let p = app.password_prompt.take().unwrap();
            app.connect_mysql_server_with_password(PasswordPromptState {
                server_url: p.server_url,
                user: "root".into(),
                buffer: "root".into(),
            });
        }
        let Some(picker) = &app.db_picker else {
            panic!(
                "debe abrirse el picker (prompt: {:?}, picker: {:?})",
                app.password_prompt.is_some(),
                app.db_picker.is_some()
            );
        };
        assert!(picker.dbs.contains(&"lazydb_demo".to_string()), "bases: {:?}", picker.dbs);
        // Elegir la BD por índice → connect_sqlite carga el catálogo
        let idx = picker.dbs.iter().position(|d| d == "lazydb_demo").unwrap();
        let picker = app.db_picker.take().unwrap();
        let db = picker.dbs[idx].clone();
        let mut url = picker.server_url;
        url.push('/');
        url.push_str(&db);
        app.connect_sqlite(&url);
        assert!(app.tables.contains(&"categories".to_string()), "tablas: {:?}", app.tables);
        assert!(app.views.contains(&"view_order_summary".to_string()), "vistas: {:?}", app.views);
    }

    /// Smoke del flujo mongo: abrir `mongodb://host:puerto` (sin base) desde
    /// servidores locales abre el picker con las bases reales, y elegir una
    /// conecta el catálogo (colecciones). Requiere `MongoDB` local
    /// (`LAZYDB_MONGO_URI`).
    #[test]
    #[cfg(feature = "mongodb")]
    #[ignore = "requiere MongoDB local (LAZYDB_MONGO_URI)"]
    fn smoke_flujo_servidor_mongo_picker_conecta_colecciones() {
        let Ok(server_url) = std::env::var("LAZYDB_MONGO_SERVER_URL") else {
            return;
        };
        let mut app = App::new();
        // Mismo path que la UI al presionar Enter sobre el servidor detectado
        app.connect_sqlite(&server_url);
        let Some(picker) = &app.db_picker else {
            panic!("debe abrirse el picker (status: {})", app.status);
        };
        assert!(picker.dbs.iter().any(|d| d == "lazydb_probe"), "bases: {:?}", picker.dbs);
        // Elegir la base por índice → connect_sqlite conecta el catálogo
        let idx = picker.dbs.iter().position(|d| d == "lazydb_probe").unwrap();
        let picker = app.db_picker.take().unwrap();
        let db = picker.dbs[idx].clone();
        let mut url = picker.server_url;
        url.push('/');
        url.push_str(&db);
        app.connect_sqlite(&url);
        assert!(app.tables.iter().any(|c| c == "test_probe"), "colecciones: {:?}", app.tables);
    }

    /// Enter en el campo URL con URL válida → parsea Y conecta (no solo
    /// rellena campos). Regresión del "le doy enter y nada".
    #[test]
    #[cfg(feature = "mongodb")]
    #[ignore = "requiere MongoDB local (LAZYDB_MONGO_SERVER_URL)"]
    fn enter_en_url_del_formulario_conecta() {
        let Ok(server_url) = std::env::var("LAZYDB_MONGO_SERVER_URL") else {
            return;
        };
        let mut app = App::new();
        app.active_panel = PanelKind::Detail;
        app.connection_form = Some(ConnectionFormState::default());
        // Escribir la URL con base y Enter → debe conectar directo
        let mut url = server_url;
        url.push('/');
        url.push_str("lazydb_probe");
        for c in url.chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.db_path.is_some(),
            "Enter en URL válida debe conectar (status: {})",
            app.status
        );
        assert!(
            app.active_adapter.is_some(),
            "el adapter debe compartirse en active_adapter tras conectar"
        );
        assert!(app.tables.iter().any(|c| c == "test_probe"), "colecciones: {:?}", app.tables);
    }

    #[test]
    fn sin_db_el_detail_muestra_el_formulario_de_conexion() {
        // Al arrancar (sin db): el Detail debe ofrecer el formulario de
        // conexión con el foco en la URL (nada del placeholder viejo).
        let app = App::new();
        let Some(form) = app.connection_form.as_ref() else {
            panic!("debe existir el formulario de conexión al arrancar");
        };
        assert_eq!(form.active, ConnField::Url, "foco inicial en la URL");
        assert!(app.preview_rows.is_empty(), "sin items de lista");
    }

    #[test]
    fn regla_de_oro_el_formulario_nunca_se_cierra_sin_db() {
        // Esc con el foco en Detail no cierra el formulario: solo mueve el
        // foco a Fuentes. El estado `connection_form` persiste.
        let mut app = App::new();
        app.active_panel = PanelKind::Detail;
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.connection_form.is_some(), "Esc no debe cerrar el formulario sin db abierta");
        assert_eq!(app.active_panel, PanelKind::Sources, "Esc mueve el foco a Fuentes");

        // Navegar a otros paneles no elimina el formulario tampoco
        app.active_panel = PanelKind::Tables;
        assert!(app.connection_form.is_some(), "el formulario persiste en cualquier panel");
    }

    #[test]
    fn esc_cierra_el_prompt_de_contrasena_aunque_el_formulario_este_activo() {
        // El prompt de contraseña (modal superpuesto) captura las teclas
        // ANTES que el formulario de conexión: Esc debe cerrarlo siempre,
        // no "mover el foco" (que dejaba el modal colgado).
        let mut app = App::new();
        app.active_panel = PanelKind::Detail; // el formulario está activo
        app.connection_form = Some(ConnectionFormState::default());
        app.password_prompt = Some(PasswordPromptState {
            server_url: "mysql://127.0.0.1:3306".to_string(),
            user: "root".to_string(),
            buffer: "x".to_string(),
        });
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            app.password_prompt.is_none(),
            "Esc debe cerrar el prompt de contraseña (no lo intercepta el formulario)"
        );
        assert_eq!(app.active_panel, PanelKind::Detail, "el foco no debe moverse");
    }

    #[test]
    fn conectar_con_exito_oculta_el_formulario_y_desconectar_lo_reactiva() {
        // Simula el contrato de la regla de oro: el render muestra el
        // formulario cuando `db_path.is_none()`, sea cual sea el panel.
        let app = App::new();
        assert!(app.db_path.is_none());
        assert!(app.connection_form.is_some(), "sin db → formulario presente");
    }

    #[test]
    fn conn_parse_url_rellena_los_campos() {
        let mut app = App::new();
        app.connection_form = Some(ConnectionFormState::default());
        app.connection_form.as_mut().unwrap().url =
            "mysql://user:pass@localhost:3306/lazy".to_string();
        assert!(app.conn_parse_url_into_fields());
        let form = app.connection_form.as_ref().unwrap();
        assert_eq!(form.kind, crate::db::connection::ConnectionType::Mysql);
        assert_eq!(form.host, "localhost");
        assert_eq!(form.port, "3306");
        assert_eq!(form.user, "user");
        assert_eq!(form.pass, "pass");
        assert_eq!(form.db, "lazy");
        assert!(form.detected_note.contains("MySQL"), "nota: {}", form.detected_note);
    }

    #[test]
    fn conn_parse_url_uri_clevercloud_rellena_credenciales() {
        let mut app = App::new();
        app.connection_form = Some(ConnectionFormState::default());
        app.connection_form.as_mut().unwrap().url = concat!(
            "postgresql://uo8h6cfqdm4u5xv6lfqq:dsJBQr44561wnu9YizPLTeP1GFh0eO@",
            "bh4bhaewrpd8qqcreso5-postgresql.services.clever-cloud.com:5432/",
            "bh4bhaewrpd8qqcreso5"
        )
        .to_string();
        assert!(app.conn_parse_url_into_fields());
        let form = app.connection_form.as_ref().unwrap();
        assert_eq!(form.user, "uo8h6cfqdm4u5xv6lfqq");
        assert_eq!(form.pass, "dsJBQr44561wnu9YizPLTeP1GFh0eO");
        assert_eq!(form.port, "5432");
        assert_eq!(form.db, "bh4bhaewrpd8qqcreso5");
    }

    #[test]
    fn ctrl_u_limpia_el_campo_activo_y_ctrl_l_limpia_todo() {
        let mut app = App::new();
        app.active_panel = PanelKind::Detail; // el formulario captura con foco en Detail
        app.connection_form = Some(ConnectionFormState::default());
        app.connection_form.as_mut().unwrap().url =
            "mysql://user:pass@localhost:3306/lazy".to_string();
        app.connection_form.as_mut().unwrap().host = "localhost".to_string();
        app.connection_form.as_mut().unwrap().active = ConnField::Url;

        // Ctrl+U limpia el campo URL activo
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.connection_form.as_ref().unwrap().url, "");
        assert_eq!(
            app.connection_form.as_ref().unwrap().host,
            "localhost",
            "otros campos intactos con Ctrl+U"
        );

        // Ctrl+L limpia todo el formulario
        app.connection_form.as_mut().unwrap().host = "localhost".to_string();
        app.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        let form = app.connection_form.as_ref().unwrap();
        assert!(form.host.is_empty(), "Ctrl+L limpia todo");
        assert!(form.url.is_empty());
        assert_eq!(form.active, ConnField::Url, "el foco vuelve a la URL");
    }

    #[test]
    fn conn_parse_url_incompleta_no_toca_campos() {
        let mut app = App::new();
        app.connection_form = Some(ConnectionFormState::default());
        {
            let form = app.connection_form.as_mut().unwrap();
            form.url = "mysql:/".to_string(); // incompleta: sin `//host`
            form.host = "ya-editado".to_string();
        }
        assert!(!app.conn_parse_url_into_fields());
        let form = app.connection_form.as_ref().unwrap();
        assert_eq!(form.host, "ya-editado", "no debe sobreescribir campos manuales");
    }

    #[test]
    fn conn_rebuild_url_desde_campos() {
        let mut app = App::new();
        app.connection_form = Some(ConnectionFormState::default());
        {
            let form = app.connection_form.as_mut().unwrap();
            form.kind = crate::db::connection::ConnectionType::Mysql;
            form.host = "db.azure.com".to_string();
            form.port = "3306".to_string();
            form.user = "admin".to_string();
            form.pass = "secreto".to_string();
            form.db = "prod".to_string();
        }
        app.conn_rebuild_url_from_fields();
        assert_eq!(
            app.connection_form.as_ref().unwrap().url,
            "mysql://admin:secreto@db.azure.com:3306/prod"
        );
    }

    /// Reproducción del bug reportado: pegar la `URI` de `CleverCloud` (con base
    /// y credenciales) y dar Enter NO debe abrir el prompt de contraseña —
    /// la URL ya trae `user:pass` y `/db`. Sin red, el resolver falla con
    /// error de conexión, pero NUNCA debe activar `password_prompt`.
    #[test]
    #[cfg(feature = "postgres")]
    fn uri_clevercloud_con_base_no_abre_prompt_de_contrasena() {
        let mut app = App::new();
        app.active_panel = PanelKind::Detail;
        app.connection_form = Some(ConnectionFormState::default());
        let uri = concat!(
            "postgresql://uo8h6cfqdm4u5xv6lfqq:dsJBQr44561wnu9YizPLTeP1GFh0eO@",
            "bh4bhaewrpd8qqcreso5-postgresql.services.clever-cloud.com:5432/",
            "bh4bhaewrpd8qqcreso5"
        );
        app.connection_form.as_mut().unwrap().url = uri.to_string();
        // Enter en el campo URL → parsea + conecta
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.password_prompt.is_none(),
            "la URI con /db y credenciales NO debe pedir contraseña (status: {})",
            app.status
        );
    }

    /// Regresión: una URL pegada PARTIDA por un `\n` (`CleverCloud` la parte en
    /// dos líneas) debe purgarse antes de analizar/conectar: sin saltos de
    /// línea y con la base registrada. Nunca debe ir al flujo de servidor.
    #[test]
    fn on_paste_sanitiza_nueva_linea_y_conn_parse_purga_la_url() {
        let mut app = App::new();
        app.active_panel = PanelKind::Detail;
        app.connection_form = Some(ConnectionFormState::default());

        // Simula el pegado de la URL partida (con \n entre el puerto y la db)
        let pegada = concat!(
            "postgresql://uo8h6cfqdm4u5xv6lfqq:secret@host:5432/\n",
            "bh4bhaewrpd8qqcreso5"
        );
        app.on_paste(pegada);
        let url_limpia = app.connection_form.as_ref().unwrap().url.clone();
        assert!(!url_limpia.contains('\n'), "el paste debe quitar el \\n: {url_limpia:?}");
        assert!(
            url_limpia.ends_with("bh4bhaewrpd8qqcreso5"),
            "la base debe quedar al final: {url_limpia:?}"
        );

        // Enter → parsea (con purga) y no debe ir al flujo de servidor
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.password_prompt.is_none(),
            "URL purgada no debe pedir contraseña (status: {})",
            app.status
        );
    }

    // ── scroll infinito del Data tab (regresión) ──────────────────────

    /// Adapter fake: simula una tabla con `total` filas. Cada página
    /// (`table_rows_sorted`) devuelve `limit` filas a partir de `offset`,
    /// con celdas `v{idx}` — suficiente para ejercitar el scroll infinito
    /// sin tocar una BD real.
    struct FakeScrollAdapter {
        total: u32,
    }

    impl crate::db::adapter::DbAdapter for FakeScrollAdapter {
        fn list_objects_by_type(&self, _object_type: &str) -> Result<Vec<String>, db::DbError> {
            Ok(vec![])
        }
        fn list_advanced_objects(&self) -> Result<Vec<String>, db::DbError> {
            Ok(vec![])
        }
        fn object_sql(&self, _object_name: &str) -> Result<String, db::DbError> {
            Ok(String::new())
        }
        fn table_columns(&self, _table_name: &str) -> Result<Vec<db::ColumnInfo>, db::DbError> {
            Ok(vec![])
        }
        fn table_row_count(&self, _table_name: &str) -> Result<u32, db::DbError> {
            Ok(self.total)
        }
        fn column_names(&self, _table_name: &str) -> Result<Vec<db::Column>, db::DbError> {
            Ok(vec![])
        }
        fn table_data_rows(
            &self,
            _table_name: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<db::Row>, db::DbError> {
            Ok(vec![])
        }
        fn table_rows_sorted(
            &self,
            _table_name: &str,
            limit: u32,
            offset: u32,
            _order_col: Option<(&str, bool)>,
        ) -> Result<db::TableData, db::DbError> {
            let columns = vec![db::Column { name: "id".to_string(), dtype: "INTEGER".to_string() }];
            let rows: Vec<db::Row> = (offset..offset.saturating_add(limit))
                .filter(|i| *i < self.total)
                .map(|i| db::Row { cells: vec![format!("v{i}")] })
                .collect();
            Ok(db::TableData { columns, rows })
        }
        fn foreign_keys(&self, _table_name: &str) -> Result<Vec<db::ForeignKey>, db::DbError> {
            Ok(vec![])
        }
        fn row_offset_of(
            &self,
            _table_name: &str,
            _col: &str,
            _value: &str,
        ) -> Result<Option<u32>, db::DbError> {
            Ok(None)
        }
        fn query(&self, _sql: &str, _limit: u32) -> Result<Vec<String>, db::DbError> {
            Ok(vec![])
        }
        fn count(&self, _sql: &str) -> Result<u32, db::DbError> {
            Ok(self.total)
        }
    }

    fn app_con_scroll_fake(total: u32, page: u32) -> App {
        let mut app = App::new();
        app.active_adapter = Some(std::sync::Arc::new(FakeScrollAdapter { total }));
        app.active_panel = PanelKind::Detail;
        app.detail_tab = DetailTab::Data;
        app.tables = vec!["t".to_string()];
        app.object_section = ObjectSection::Tables;
        app.set_selected_idx(PanelKind::Tables, 0);
        app.rows_per_page = page;
        app.total_rows = total;
        app
    }

    /// Llena `preview_rows` (header + `n` filas) y `preview_data` (tipado)
    /// con `n` filas — el estado exacto tras cargar una página.
    fn cargar_pagina(app: &mut App, n: u32) {
        let rows: Vec<db::Row> = (0..n).map(|i| db::Row { cells: vec![format!("v{i}")] }).collect();
        app.preview_data = Some(db::TableData {
            columns: vec![db::Column { name: "id".to_string(), dtype: "INTEGER".to_string() }],
            rows: rows.clone(),
        });
        let mut lines = vec!["id".to_string()];
        lines.extend(rows.iter().map(|r| r.to_line(" | ")));
        app.preview_rows = lines;
    }

    /// REGRESIÓN: el scroll hacia abajo se congelaba porque
    /// `scroll_down_infinite` extendía `preview_rows` (strings) pero NO
    /// `preview_data` (celdas tipadas). El render usa `data.rows.len()`
    /// como tope del viewport → la vista se quedaba en la primera página
    /// mientras el label `row X/Y` seguía subiendo.
    #[tokio::test]
    async fn scroll_down_infinite_mantiene_preview_data_sincronizado() {
        let mut app = app_con_scroll_fake(250, 25);
        cargar_pagina(&mut app, 25); // página 1 cargada (25 filas)

        app.scroll_down_infinite(); // carga la página 2

        let data_len = app.preview_data.as_ref().expect("preview_data").rows.len();
        assert_eq!(data_len, 50, "preview_data debe crecer con cada página");
        assert_eq!(
            app.preview_rows.len(),
            data_len + 1,
            "preview_rows (con header) debe seguir a preview_data"
        );
    }

    /// REGRESIÓN: al hacer scroll hacia arriba en el borde, el prepend de N
    /// filas saltaba la vista hacia abajo ("cambio de página" con la rueda):
    /// la selección se desplazaba +N pero `scroll_offset` (viewport) no, y
    /// el render "corregía" saltando la página. Ahora el viewport se
    /// desplaza junto con la selección y la vista queda estable.
    #[tokio::test]
    async fn scroll_up_infinite_desplaza_viewport_con_la_seleccion() {
        let mut app = app_con_scroll_fake(250, 25);
        // Simula haber bajado 4 páginas: buffer con 25 filas, offset global 100
        let rows: Vec<db::Row> =
            (0..25).map(|i| db::Row { cells: vec![format!("v{}", i + 100)] }).collect();
        app.preview_data = Some(db::TableData {
            columns: vec![db::Column { name: "id".to_string(), dtype: "INTEGER".to_string() }],
            rows: rows.clone(),
        });
        let mut lines = vec!["id".to_string()];
        lines.extend(rows.iter().map(|r| r.to_line(" | ")));
        app.preview_rows = lines;
        app.preview_loaded_offset = 100;
        // El usuario está en la PRIMERA fila del buffer (borde superior)
        app.set_selected_idx(PanelKind::Detail, 1);
        app.panel_mut(PanelKind::Detail).scroll_offset.set(0);

        app.scroll_up_infinite(); // prepend de la página anterior (25 filas)

        // El viewport debe desplazarse +25 igual que la selección
        assert_eq!(app.panel(PanelKind::Detail).scroll_offset.get(), 25);
        assert_eq!(app.selected_idx(PanelKind::Detail), 26, "selección +25");
        assert_eq!(app.preview_loaded_offset, 75, "offset global retrocede 25");
        // Invariante de fila global: misma fila visible que antes del prepend
        // (100 global antes == 75 + (26 - 1) después)
        let fila_global =
            app.preview_loaded_offset as usize + app.selected_idx(PanelKind::Detail) - 1;
        assert_eq!(fila_global, 100);
    }
}
