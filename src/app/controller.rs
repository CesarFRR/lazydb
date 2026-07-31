use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Rect;

use crate::app::panel::{Panel, PanelKind, PanelMode};
use crate::ui::layout::{self, ComputedLayout};
use crate::ui::widgets::panel::MIN_COL_W;
use crate::{config, db, keys, query, storage};

#[allow(dead_code)]
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
enum DragState {
    /// Arrastrando la barra de scroll horizontal del Data tab
    HScroll,
    /// Arrastrando el scrollbar vertical de un panel
    VScroll(PanelKind),
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

fn is_online_source(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ssh://")
        || lower.starts_with("postgres://")
        || lower.starts_with("mysql://")
        || lower.starts_with("sqlite://")
}

fn is_local_source(value: &str) -> bool {
    !is_online_source(value)
}

// ── formato de items del panel Fuentes ─────────────────────────────────
// Cada item es un string plano con marcas que el render colorea:
//   sección:   "\u{1}LABEL"          (marcador interno, no visible)
//   entry:     [● ]<★|▣|⊙ ><texto>   (● = conectada, ★ = favorito,
//                                     ▣ = sqlite local, ⊙ = online)
// Los favoritos usan "name => path"; el resto muestra el path directo.

/// Marcador interno de sección (SOH): nunca se renderiza tal cual.
pub const SOURCE_SECTION_MARK: char = '\u{1}';

fn source_section(label: &str) -> String {
    format!("{SOURCE_SECTION_MARK}{label}")
}

fn is_source_section(item: &str) -> bool {
    item.starts_with(SOURCE_SECTION_MARK)
}

/// Quita las marcas decorativas (● ★ ▣ ⊙, combinables: "● ★ x") de un item de
/// Fuentes y devuelve el dato real (path o "name => path" para favoritos).
fn strip_source_marks(mut item: &str) -> &str {
    loop {
        let mut stripped = false;
        for mark in ["● ", "★ ", "▣ ", "⊙ "] {
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
            Self::Local => is_local_source(path),
            Self::Online => is_online_source(path),
        }
    }
}

/// Construye la lista del panel Fuentes por secciones: FAVORITOS, RECIENTES
/// y LOCAL DETECTADO (./), con marcas de tipo y de DB conectada.
struct SourceList<'a> {
    state: &'a storage::AppState,
    connected: Option<&'a str>,
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
        let prefix = if is_fav {
            "★ "
        } else if is_online_source(&path) {
            "⊙ "
        } else {
            "▣ "
        };
        let mark = if self.connected == Some(path.as_str()) { "● " } else { "" };
        match display {
            Some(name) => self.out.push(format!("{mark}{prefix}{name} => {path}")),
            None => self.out.push(format!("{mark}{prefix}{path}")),
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
        if favs.is_empty() {
            return;
        }
        self.section("FAVORITOS");
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

    fn add_detected(&mut self) {
        // DBs SQLite de la carpeta actual (donde se ejecuta lazydb)
        let scanned = scan_cwd_databases();
        let fresh: Vec<String> =
            scanned.iter().filter(|p| !self.seen.contains(*p)).cloned().collect();
        if fresh.is_empty() {
            return;
        }
        self.section("LOCAL DETECTADO (./)");
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
/// lazydb) buscando archivos de base de datos `SQLite`: `*.db`, `*.sqlite` y
/// `*.sqlite3`. Devuelve los paths completos ordenados alfabéticamente.
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
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "db" | "sqlite" | "sqlite3"))
        })
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .collect();
    dbs.sort();
    dbs
}

// ---------------------------------------------------------------------------
// App (estado global)
// ---------------------------------------------------------------------------

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
    pub source_tab: SourceTab,
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub advanced: Vec<String>,
    pub preview_rows: Vec<String>,
    pub detail_tab: DetailTab,

    // ── estado persistente ──
    pub should_quit: bool,
    pub is_loading: bool,
    pub refresh_count: u32,
    pub db_path: Option<String>,
    pub db_size_bytes: Option<u64>,
    pub status: String,
    pub current_page: u32,
    pub rows_per_page: u32,
    pub total_rows: u32,
    /// Índice 0-based en el dataset de `preview_rows[1]` (primera fila de datos)
    pub preview_loaded_offset: u32,
    pub query_state: query::QueryState,
    pub query_results: Vec<String>,
    pub state: storage::AppState,
    pub keymap: keys::Keymap,
    pub show_actions_menu: bool,
    pub actions_menu_idx: usize,
    /// Inspector de fila (modal de detalle de registro)
    pub show_row_inspector: bool,
    pub row_inspector_pairs: Vec<(String, String)>,
    pub inspector_scroll: crate::ui::widgets::modal::ModalScroll,
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
        let ui_config = config::load_ui_config();
        let source_tab = SourceTab::All;

        let panels = [
            Panel::new_sidebar(PanelKind::Sources),
            Panel::new_sidebar(PanelKind::Tables),
            Panel::new_sidebar(PanelKind::Views),
            Panel::new_sidebar(PanelKind::Advanced),
            Panel::new(PanelKind::Detail),
        ];

        Self {
            panels,
            active_panel: PanelKind::Sources,
            last_sidebar_focus: PanelKind::Sources,
            layout: ComputedLayout::default(),
            sources: Self::build_sources(&state, source_tab, None),
            source_tab,
            tables: vec![],
            views: vec![],
            advanced: vec![],
            preview_rows: vec!["Sin conexion SQLite".to_string()],
            detail_tab: DetailTab::Data,
            should_quit: false,
            is_loading: false,
            refresh_count: 0,
            db_path: None,
            db_size_bytes: None,
            status: "Sin conexion SQLite".to_string(),
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
            inspector_scroll: crate::ui::widgets::modal::ModalScroll::default(),
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
        }
    }

    // ── layout ────────────────────────────────────────────────────────

    pub fn compute_layout(&mut self, width: u16, height: u16) {
        let mode_overrides = self.panels.iter().map(|p| (p.kind, p.mode)).collect::<Vec<_>>();
        let modes: [(PanelKind, PanelMode); 5] = {
            let mut arr = [(PanelKind::Sources, PanelMode::default()); 5];
            for (i, &(k, m)) in mode_overrides.iter().enumerate() {
                if i < 5 {
                    arr[i] = (k, m);
                }
            }
            arr
        };

        let active_sidebar = if self.active_panel.is_sidebar() {
            self.active_panel
        } else {
            self.last_sidebar_focus
        };

        self.layout = layout::compute(width, height, active_sidebar, self.active_panel, &modes);
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
                        // Número de fila actual / total
                        if self.total_rows > 0 {
                            let current_row = self.preview_loaded_offset
                                + self.selected_idx(PanelKind::Detail).saturating_sub(1) as u32
                                + 1;
                            let total = self.total_rows;
                            format!("{label} - row {current_row}/{total}")
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
    ) -> Vec<String> {
        let mut list = SourceList {
            state,
            connected,
            out: Vec::new(),
            seen: HashSet::new(),
            sections: HashSet::new(),
        };

        match source_tab {
            SourceTab::All => {
                list.add_favs(SourceFilter::All);
                list.add_recents(SourceFilter::All);
                list.add_detected();
            }
            SourceTab::Local => {
                list.add_favs(SourceFilter::Local);
                list.add_recents(SourceFilter::Local);
                list.add_detected();
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
        self.sources = Self::build_sources(&self.state, self.source_tab, self.db_path.as_deref());
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

    pub fn db_path_display(&self) -> &str {
        self.db_path.as_deref().unwrap_or("-")
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

    // ── conexión SQLite ───────────────────────────────────────────────

    fn connect_sqlite(&mut self, path: &str) {
        // Choke point de normalización: cualquier ruta que entre aquí queda
        // canónica (absoluta, sin ./ ni ..), igual que la que produce el
        // escaneo. Sin esto la misma DB aparece duplicada en Fuentes y la
        // marca ● de "conectada" nunca coincide (relativo vs absoluto).
        let path = crate::paths::normalize_path(path);
        self.is_loading = true;
        self.status = format!("Conectando a {path}...");
        let tables = db::backends::sqlite::list_objects_by_type(&path, "table");
        let views = db::backends::sqlite::list_objects_by_type(&path, "view");
        let advanced = db::backends::sqlite::list_advanced_objects(&path);

        if let (Ok(tables), Ok(views), Ok(advanced)) = (tables, views, advanced) {
            let path_str = path.clone();
            self.state.add_recent(path_str);
            let _ = self.state.save();
            self.sources = Self::build_sources(&self.state, self.source_tab, Some(&path));

            self.db_path = Some(path.clone());
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
            self.status = format!("Conectado en modo read-only: {path}");

            // Mover foco a Tablas
            self.set_focus(PanelKind::Tables);
        } else {
            self.is_loading = false;
            self.status = format!("Error al abrir {path}: no se pudo leer sqlite_master");
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
        // Ajustar rows_per_page dinámicamente según espacio disponible
        self.rows_per_page = self.optimal_rows_per_page();
        // Reset scroll_offset del viewport al recargar completamente los datos
        self.panel_mut(PanelKind::Detail).scroll_offset.set(0);
        // Reset scroll horizontal: el nuevo objeto puede tener otras columnas
        self.panel_mut(PanelKind::Detail).h_scroll.set(0);
        let Some(path) = self.db_path.as_deref() else {
            return;
        };

        self.is_loading = true;
        self.status = format!("Cargando {}...", self.detail_tab.label().trim());

        let object_name = self.selected_object_name();
        if object_name.is_empty() || object_name == "-" {
            self.preview_rows = vec!["Sin objeto seleccionado".to_string()];
            self.total_rows = 0;
            self.preview_loaded_offset = 0;
            self.is_loading = false;
            self.set_selected_idx(PanelKind::Detail, 0);
            return;
        }

        // Siempre refrescar total_rows para tablas/vistas (no Advanced)
        if self.object_section != ObjectSection::Advanced {
            if let Ok(count) = db::backends::sqlite::table_row_count(path, &object_name) {
                self.total_rows = count;
            }
        }

        match self.detail_tab {
            DetailTab::Data => {
                if self.object_section == ObjectSection::Advanced {
                    // Para índices/triggers: mostrar el SQL DDL
                    match db::backends::sqlite::object_sql(path, &object_name) {
                        Ok(sql) => {
                            self.preview_rows =
                                sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                            if self.preview_rows.is_empty() {
                                self.preview_rows = vec!["-- SQL vacio --".to_string()];
                            }
                        }
                        Err(err) => {
                            self.preview_rows = vec![format!("Error SQL: {err}")];
                        }
                    }
                    self.total_rows = 0;
                    self.preview_loaded_offset = 0;
                    self.is_loading = false;
                    self.set_selected_idx(PanelKind::Detail, 0);
                    return;
                }

                match db::backends::sqlite::table_row_count(path, &object_name) {
                    Ok(_) => {} // total_rows ya fue actualizado arriba
                    Err(err) => {
                        self.preview_rows = vec![format!("Error contando filas: {err}")];
                        self.total_rows = 0;
                        self.preview_loaded_offset = 0;
                        self.is_loading = false;
                        self.set_selected_idx(PanelKind::Detail, 0);
                        return;
                    }
                }

                let offset = self.current_page.saturating_mul(self.rows_per_page);
                let order_col = self.sort_column.as_deref().map(|col| (col, self.sort_asc));
                match db::backends::sqlite::table_rows_sorted(
                    path,
                    &object_name,
                    self.rows_per_page,
                    offset,
                    order_col,
                ) {
                    Ok(rows) => {
                        self.preview_rows =
                            if rows.is_empty() { vec!["<sin datos>".to_string()] } else { rows };
                        self.preview_loaded_offset = offset;
                        self.set_selected_idx(
                            PanelKind::Detail,
                            usize::from(self.preview_rows.len() > 1),
                        );
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error obteniendo filas: {err}")];
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                }
            }
            DetailTab::Schema => {
                if self.object_section == ObjectSection::Advanced {
                    // Schema de índice/trigger = su SQL DDL
                    match db::backends::sqlite::object_sql(path, &object_name) {
                        Ok(sql) => {
                            self.preview_rows =
                                sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                            if self.preview_rows.is_empty() {
                                self.preview_rows = vec!["-- SQL vacio --".to_string()];
                            }
                        }
                        Err(err) => {
                            self.preview_rows = vec![format!("Error SQL: {err}")];
                        }
                    }
                    self.total_rows = 0;
                    self.preview_loaded_offset = 0;
                    self.is_loading = false;
                    self.set_selected_idx(PanelKind::Detail, 0);
                    return;
                }

                match db::backends::sqlite::table_columns(path, &object_name) {
                    Ok(columns) => {
                        self.preview_rows = if columns.is_empty() {
                            vec!["Sin columnas visibles".to_string()]
                        } else {
                            columns
                        };
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error schema: {err}")];
                        self.preview_loaded_offset = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                }
            }
            DetailTab::Sql => match db::backends::sqlite::object_sql(path, &object_name) {
                Ok(sql) => {
                    self.preview_rows = sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                    if self.preview_rows.is_empty() {
                        self.preview_rows = vec!["-- SQL vacio --".to_string()];
                    }
                    self.preview_loaded_offset = 0;
                    self.set_selected_idx(PanelKind::Detail, 0);
                }
                Err(err) => {
                    self.preview_rows = vec![format!("Error SQL: {err}")];
                    self.preview_loaded_offset = 0;
                    self.set_selected_idx(PanelKind::Detail, 0);
                }
            },
            DetailTab::Meta => {
                self.preview_rows = vec![
                    format!("db_path: {}", self.db_path_display()),
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
    /// Solo para tabla de datos (`DetailTab::Data`). Actualiza la selección
    /// para que apunte a la primera fila recién cargada (continuidad hacia abajo).
    fn scroll_down_infinite(&mut self) {
        let data_len = self.preview_rows.len().saturating_sub(1);
        let next_offset = self.preview_loaded_offset as usize + data_len;
        if next_offset >= self.total_rows as usize {
            return; // ya estamos al final del dataset
        }

        let Some(path) = self.db_path.as_deref() else {
            return;
        };
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
        #[allow(clippy::cast_possible_truncation)]
        if let Ok(rows) = crate::db::backends::sqlite::table_rows_sorted(
            path,
            &object,
            limit,
            next_offset as u32,
            order_col,
        ) {
            // rows[0] es header (lo descartamos, ya tenemos el nuestro)
            // rows[1..] son las filas de datos nuevas
            if rows.len() <= 1 {
                self.is_loading = false;
                return;
            }
            let old_len = self.preview_rows.len();
            self.preview_rows.extend(rows.iter().skip(1).cloned());
            // La selección va a la primera fila nueva (continuidad: se avanzó 1 paso)
            self.set_selected_idx(PanelKind::Detail, old_len);
        }
        self.is_loading = false;
    }

    /// Carga la página anterior de datos y la antepone a `preview_rows`.
    /// Solo para tabla de datos (`DetailTab::Data`). Actualiza `preview_loaded_offset`
    /// y la selección para que apunte a la última fila recién cargada
    /// (continuidad hacia arriba).
    fn scroll_up_infinite(&mut self) {
        if self.preview_loaded_offset == 0 {
            return; // ya estamos al inicio del dataset
        }

        let Some(path) = self.db_path.as_deref() else {
            return;
        };
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
        if let Ok(rows) =
            crate::db::backends::sqlite::table_rows_sorted(path, &object, limit, offset, order_col)
        {
            // rows[0] es header (lo descartamos), rows[1..] son datos nuevos
            if rows.len() <= 1 {
                self.is_loading = false;
                return;
            }
            let n = rows.len() - 1; // cantidad de filas nuevas

            // Anteponer las filas nuevas (preservando el header en index 0)
            let header = self.preview_rows[0].clone();
            let mut expanded = vec![header];
            expanded.extend(rows.iter().skip(1).cloned());
            expanded.extend(self.preview_rows.iter().skip(1).cloned());
            self.preview_rows = expanded;

            // Actualizar offset
            #[allow(clippy::cast_possible_truncation)]
            {
                self.preview_loaded_offset -= n as u32;
            }

            // La selección va a la última fila recién cargada
            // (el ítem justo antes del antiguo primer dato)
            self.set_selected_idx(PanelKind::Detail, n);
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
        let Some(path) = self.db_path.as_deref() else {
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

        self.query_state = query::QueryState::Running;
        self.status = "Ejecutando query...".to_string();

        match std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sqlite3 \"{path}\" \"{sql}\""))
            .output()
        {
            Ok(output) => {
                let result = String::from_utf8_lossy(&output.stdout);
                let count: String = result.trim().to_string();
                self.query_results = vec![format!("COUNT(*) = {count}"), format!("SQL: {sql}")];
                self.query_state = query::QueryState::Done(self.query_results.clone());
                self.status = format!("Query completada: {count} filas");
            }
            Err(e) => {
                self.query_state = query::QueryState::Error(e.to_string());
                self.status = format!("Error ejecutando query: {e}");
            }
        }
    }

    fn clear_query_state(&mut self) {
        self.query_state = query::QueryState::Idle;
        self.query_results.clear();
        self.status = "Query limpia".to_string();
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
        self.sources = Self::build_sources(&self.state, self.source_tab, self.db_path.as_deref());
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
        self.sources = Self::build_sources(&self.state, self.source_tab, self.db_path.as_deref());
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
            self.sources =
                Self::build_sources(&self.state, self.source_tab, self.db_path.as_deref());
            self.status = format!("Fuente olvidada: {path}");
        }
    }

    /// Cierra la conexión actual y vuelve el foco a Fuentes.
    fn disconnect_db(&mut self) {
        self.db_path = None;
        self.db_size_bytes = None;
        self.tables.clear();
        self.views.clear();
        self.advanced.clear();
        self.preview_rows = vec!["Sin conexion SQLite".to_string()];
        self.total_rows = 0;
        self.preview_loaded_offset = 0;
        self.current_page = 0;
        self.detail_tab = DetailTab::Data;
        self.query_state = query::QueryState::Idle;
        self.query_results.clear();
        self.sources = Self::build_sources(&self.state, self.source_tab, None);
        self.set_focus(PanelKind::Sources);
        self.set_selected_idx(PanelKind::Sources, 0);
        self.status = "Base de datos cerrada".to_string();
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
                    self.status = format!("Error exportando: {err}");
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
                        self.status = format!("Error escribiendo {filename}: {e}");
                    }
                }
            }
            Err(e) => {
                self.status = format!("Error ejecutando sqlite3: {e}");
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
        let Some(path) = self.db_path.as_deref() else {
            return;
        };
        let object = self.selected_object_name();
        if object.is_empty() || object == "-" {
            return;
        }
        let Ok(columns) = crate::db::backends::sqlite::column_names(path, &object) else {
            return;
        };

        let row_idx = self.selected_idx(PanelKind::Detail).saturating_sub(1); // skip header
        #[allow(clippy::cast_possible_truncation)]
        let offset = self.preview_loaded_offset + row_idx as u32;
        let Ok(rows) = crate::db::backends::sqlite::table_data_rows(path, &object, 1, offset)
        else {
            return;
        };

        let values: Vec<&str> = rows.first().map_or("", String::as_str).split('|').collect();

        self.row_inspector_pairs = columns
            .iter()
            .zip(values.iter().chain(std::iter::repeat(&"")))
            .map(|(col, val)| (col.clone(), val.to_string()))
            .collect();
        // Al cambiar de fila, empezar el scroll del modal desde arriba
        self.inspector_scroll.reset();
    }

    #[allow(clippy::missing_const_for_fn)]
    fn close_row_inspector(&mut self) {
        self.show_row_inspector = false;
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
        self.sources = Self::build_sources(&self.state, self.source_tab, self.db_path.as_deref());

        let ui_config = config::load_ui_config();
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
            s if s.contains(" => ") => {
                let path = s.split_once(" => ").map(|(_, p)| p.to_string()).unwrap_or_default();
                self.connect_sqlite(&path);
            }
            s if s.starts_with('/')
                || std::path::Path::new(s)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("db")) =>
            {
                self.connect_sqlite(s);
            }
            _ => {
                self.status = format!("No se puede conectar: {clean}");
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
        } else {
            self.should_quit = true;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // ── modo filtro: capturar teclas antes del mapeo de acciones ──
        if self.input_mode == InputMode::Filtering {
            self.handle_filter_key(key);
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
            keys::AppAction::ToggleActionsMenu => {
                self.show_actions_menu = true;
                self.actions_menu_idx = 0;
                self.status = "Menu de acciones abierto".to_string();
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
                    let current_row = self.preview_loaded_offset
                        + self.selected_idx(PanelKind::Detail) as u32
                        - 1;
                    let new_row = current_row.saturating_sub(self.rows_per_page);
                    self.current_page = new_row / self.rows_per_page;
                    self.refresh_preview_from_selected_object();
                }
            }
            keys::AppAction::NextPage => {
                if self.active_panel == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    let current_row = self.preview_loaded_offset
                        + self.selected_idx(PanelKind::Detail) as u32
                        - 1;
                    let new_row =
                        (current_row + self.rows_per_page).min(self.total_rows.saturating_sub(1));
                    self.current_page = new_row / self.rows_per_page;
                    self.refresh_preview_from_selected_object();
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
            // Panel NO enfocado: solo desplazar vista sin cambiar foco
            let items_len = self.items_len_for(target);
            let old_idx = self.selected_idx(target);

            // Actualizar scroll y selección
            {
                let p = self.panel_mut(target);
                if up {
                    p.scroll_offset.set(p.scroll_offset.get().saturating_sub(1));
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

    pub fn on_mouse_click(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width < 40 || height < 10 {
            return;
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
                // Click dentro del modal: sin acción
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
        assert_eq!(strip_source_marks("★ one => /a/one.db"), "one => /a/one.db");
        assert_eq!(strip_source_marks("Abrir sakila.db"), "Abrir sakila.db");
        // Marcas combinables (conectada + tipo)
        assert_eq!(strip_source_marks("● ▣ /tmp/x.db"), "/tmp/x.db");
        assert_eq!(strip_source_marks("● ★ one => /a/one.db"), "one => /a/one.db");
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
        let sources = App::build_sources(&state, SourceTab::Online, None);

        assert!(sources.iter().any(|s| s == &source_section("FAVORITOS")));
        assert!(sources.iter().any(|s| s.starts_with("★ remote => https://remote.example/db")));
        // "one" es favorito local → no aparece en el tab Online
        assert!(!sources.iter().any(|s| s.contains("/a/one.db")));
        // Sin acciones fijas en Online
        assert!(!sources.iter().any(|s| s == "Abrir sakila.db"));
        assert!(!sources.iter().any(|s| s == "Buscar archivo .db"));
    }

    #[test]
    fn build_sources_marca_la_conectada() {
        let state = state_de_prueba();
        let sources = App::build_sources(&state, SourceTab::All, Some("/a/one.db"));
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
        let sources = App::build_sources(&state, SourceTab::All, None);

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
        let sources = App::build_sources(&state, SourceTab::All, Some(&connected));
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
        let sources = App::build_sources(&state, SourceTab::All, None);
        let shown = sources.iter().find_map(|s| {
            let p = source_path_of(s);
            (!is_source_section(s) && p != "Abrir sakila.db" && p != "Buscar archivo .db")
                .then_some(p)
        });
        let canonical = crate::paths::normalize_path("one.db");
        assert_eq!(shown, Some(canonical.as_str()));
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
}
