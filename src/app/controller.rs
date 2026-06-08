use std::collections::HashSet;

use crossterm::event::KeyEvent;
use ratatui::prelude::Rect;

use crate::app::panel::{Panel, PanelKind, PanelMode};
use crate::ui::layout::{self, ComputedLayout};
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

// ---------------------------------------------------------------------------
// App (estado global)
// ---------------------------------------------------------------------------

enum EnterAction {
    Connect(String),
    UpdateStatus,
    None,
}

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
    pub refresh_count: u32,
    pub db_path: Option<String>,
    pub db_size_bytes: Option<u64>,
    pub status: String,
    pub current_page: u32,
    pub rows_per_page: u32,
    pub total_rows: u32,
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
            sources: Self::build_sources(&state, source_tab),
            source_tab,
            tables: vec![],
            views: vec![],
            advanced: vec![],
            preview_rows: vec!["Sin conexion SQLite".to_string()],
            detail_tab: DetailTab::Data,
            should_quit: false,
            refresh_count: 0,
            db_path: None,
            db_size_bytes: None,
            status: "Sin conexion SQLite".to_string(),
            current_page: 0,
            rows_per_page: ui_config.rows_per_page,
            total_rows: 0,
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
                Self::shift_index_on_vec_len(
                    &mut self.panel_mut(PanelKind::Detail).selected_idx,
                    len,
                    step,
                );
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

    // ── items por panel ───────────────────────────────────────────────

    pub fn items_for(&self, kind: PanelKind) -> &[String] {
        match kind {
            PanelKind::Sources => &self.sources,
            PanelKind::Tables => &self.tables,
            PanelKind::Views => &self.views,
            PanelKind::Advanced => &self.advanced,
            PanelKind::Detail => &self.preview_rows,
        }
    }

    pub fn items_len_for(&self, kind: PanelKind) -> usize {
        self.items_for(kind).len()
    }

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
                        // Page info siempre al lado de Datos
                        if self.total_rows > 0 {
                            let total = self.total_rows.div_ceil(self.rows_per_page).max(1);
                            format!("{label} - P{}/{}", self.current_page + 1, total)
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

    fn build_sources(state: &storage::AppState, source_tab: SourceTab) -> Vec<String> {
        let mut sources = Vec::new();
        let mut seen = HashSet::new();

        let mut push_unique = |value: String, out: &mut Vec<String>| {
            if seen.insert(value.clone()) {
                out.push(value);
            }
        };

        match source_tab {
            SourceTab::All => {
                for recent in &state.recents {
                    push_unique(recent.clone(), &mut sources);
                }
                let mut favorites = state
                    .favorites
                    .iter()
                    .map(|(name, path)| format!("{name} => {path}"))
                    .collect::<Vec<_>>();
                favorites.sort();
                for fav in favorites {
                    push_unique(fav, &mut sources);
                }
            }
            SourceTab::Local => {
                for recent in &state.recents {
                    if is_local_source(recent) {
                        push_unique(recent.clone(), &mut sources);
                    }
                }
                let mut favorites = state.favorites.iter().collect::<Vec<_>>();
                favorites.sort_by(|a, b| a.0.cmp(b.0));
                for (name, path) in favorites {
                    if is_local_source(path) {
                        push_unique(format!("{name} => {path}"), &mut sources);
                    }
                }
            }
            SourceTab::Online => {
                for recent in &state.recents {
                    if is_online_source(recent) {
                        push_unique(recent.clone(), &mut sources);
                    }
                }
                let mut favorites = state.favorites.iter().collect::<Vec<_>>();
                favorites.sort_by(|a, b| a.0.cmp(b.0));
                for (name, path) in favorites {
                    if is_online_source(path) {
                        push_unique(format!("{name} => {path}"), &mut sources);
                    }
                }
            }
        }

        if sources.is_empty() {
            sources.push("<sin entradas>".to_string());
        }

        sources.push("Buscar archivo .db".to_string());
        sources.push("Abrir sakila.db".to_string());
        sources
    }

    fn set_source_tab(&mut self, tab: SourceTab) {
        self.source_tab = tab;
        self.sources = Self::build_sources(&self.state, self.source_tab);
        self.set_selected_idx(PanelKind::Sources, 0);
    }

    // ── objetos ───────────────────────────────────────────────────────

    fn selected_object_name(&self) -> String {
        // Usar object_section (persiste) en vez de active_panel (cambia con foco)
        let section = self.object_section;

        if section == ObjectSection::Advanced {
            let items = &self.advanced;
            let idx = self.selected_idx(PanelKind::Advanced);
            let raw = items.get(idx).map_or("-", String::as_str);
            if let Some((_, name)) = raw.split_once(':') {
                return name.to_string();
            }
            return raw.to_string();
        }

        let items = match section {
            ObjectSection::Tables => &self.tables,
            ObjectSection::Views => &self.views,
            ObjectSection::Advanced => &self.advanced,
        };
        let idx = match section {
            ObjectSection::Tables => self.selected_idx(PanelKind::Tables),
            ObjectSection::Views => self.selected_idx(PanelKind::Views),
            ObjectSection::Advanced => self.selected_idx(PanelKind::Advanced),
        };
        items.get(idx).map_or_else(|| "-".to_string(), String::clone)
    }

    #[allow(dead_code)]
    pub fn selected_source(&self) -> &str {
        let idx = self.selected_idx(PanelKind::Sources);
        self.sources.get(idx).map_or("-", String::as_str)
    }

    #[allow(dead_code)]
    pub fn selected_object(&self) -> &str {
        let raw = match self.active_panel {
            PanelKind::Tables => self.tables.get(self.selected_idx(PanelKind::Tables)),
            PanelKind::Views => self.views.get(self.selected_idx(PanelKind::Views)),
            PanelKind::Advanced => self.advanced.get(self.selected_idx(PanelKind::Advanced)),
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
        let tables = db::backends::sqlite::list_objects_by_type(path, "table");
        let views = db::backends::sqlite::list_objects_by_type(path, "view");
        let advanced = db::backends::sqlite::list_advanced_objects(path);

        match (tables, views, advanced) {
            (Ok(tables), Ok(views), Ok(advanced)) => {
                let path_str = path.to_string();
                self.state.add_recent(path_str);
                let _ = self.state.save();
                self.sources = Self::build_sources(&self.state, self.source_tab);

                self.db_path = Some(path.to_string());
                self.db_size_bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
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
                self.detail_tab = DetailTab::Data;

                self.refresh_preview_from_selected_object();
                self.status = format!("Conectado en modo read-only: {path}");

                // Mover foco a Tablas
                self.set_focus(PanelKind::Tables);
            }
            _ => {
                self.status = format!("Error al abrir {path}: no se pudo leer sqlite_master");
            }
        }
    }

    fn refresh_from_connection(&mut self) {
        if let Some(path) = self.db_path.clone() {
            self.connect_sqlite(&path);
        }
    }

    // ── preview ───────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    fn refresh_preview_from_selected_object(&mut self) {
        let Some(path) = self.db_path.as_deref() else {
            return;
        };

        let object_name = self.selected_object_name();
        if object_name.is_empty() || object_name == "-" {
            self.preview_rows = vec!["Sin objeto seleccionado".to_string()];
            self.total_rows = 0;
            self.set_selected_idx(PanelKind::Detail, 0);
            return;
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
                    self.set_selected_idx(PanelKind::Detail, 0);
                    return;
                }

                match db::backends::sqlite::table_row_count(path, &object_name) {
                    Ok(count) => {
                        self.total_rows = count;
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error contando filas: {err}")];
                        self.total_rows = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                        return;
                    }
                }

                let offset = self.current_page.saturating_mul(self.rows_per_page);
                match db::backends::sqlite::table_rows(
                    path,
                    &object_name,
                    self.rows_per_page,
                    offset,
                ) {
                    Ok(rows) => {
                        self.preview_rows =
                            if rows.is_empty() { vec!["<sin datos>".to_string()] } else { rows };
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error obteniendo filas: {err}")];
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
                        self.total_rows = 0;
                        self.set_selected_idx(PanelKind::Detail, 0);
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error schema: {err}")];
                        self.total_rows = 0;
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
                    self.total_rows = 0;
                    self.set_selected_idx(PanelKind::Detail, 0);
                }
                Err(err) => {
                    self.preview_rows = vec![format!("Error SQL: {err}")];
                    self.total_rows = 0;
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
                    format!("page: {}", self.current_page + 1),
                    format!("estimated_rows: {}", self.total_rows),
                ];
                self.total_rows = 0;
                self.set_selected_idx(PanelKind::Detail, 0);
            }
        }
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
        self.sources = Self::build_sources(&self.state, self.source_tab);
        self.status = format!("Favorito guardado: {favorite_name}");
    }

    // ── menú de acciones ──────────────────────────────────────────────

    // ── row inspector ─────────────────────────────────────────────────

    fn open_row_inspector(&mut self) {
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
        let offset = self.current_page.saturating_mul(self.rows_per_page) + row_idx as u32;
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

        self.inspector_scroll.reset();
        self.show_row_inspector = true;
    }

    #[allow(clippy::missing_const_for_fn)]
    fn close_row_inspector(&mut self) {
        self.show_row_inspector = false;
    }

    /// Copia el ítem seleccionado al portapapeles del sistema.
    fn yank_selected(&mut self) {
        let items = self.items_for(self.active_panel);
        let idx = self.selected_idx(self.active_panel);
        let text = items.get(idx).cloned().unwrap_or_default();

        if text.is_empty() {
            self.status = "Nada que copiar".to_string();
            return;
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
        self.sources = Self::build_sources(&self.state, self.source_tab);

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

        if selected == "<sin entradas>" {
            self.status = "No hay elementos en esta sección".to_string();
            return;
        }

        match selected.as_str() {
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
            _ => {}
        }
    }

    // ── enter en Sources ──────────────────────────────────────────────

    fn handle_enter(&mut self) {
        if self.active_panel != PanelKind::Sources {
            return;
        }

        let selected = self.selected_source().to_string();

        if selected == "<sin entradas>" {
            self.status = "No hay elementos en esta sección".to_string();
            return;
        }

        let action = match selected.as_str() {
            "Abrir sakila.db" => EnterAction::Connect("sakila.db".to_string()),
            "Buscar archivo .db" => EnterAction::UpdateStatus,
            s if s.contains(" => ") => {
                let path =
                    s.split_once(" => ").map(|(_, path)| path.to_string()).unwrap_or_default();
                EnterAction::Connect(path)
            }
            s if s.starts_with('/')
                || std::path::Path::new(s)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("db")) =>
            {
                EnterAction::Connect(s.to_string())
            }
            _ => EnterAction::None,
        };

        match action {
            EnterAction::Connect(path) => self.connect_sqlite(&path),
            EnterAction::UpdateStatus => {
                self.status = "Buscador de archivos .db no implementado todavia".to_string();
            }
            EnterAction::None => {}
        }
    }

    // ── keyboard ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    pub fn on_key(&mut self, key: KeyEvent) {
        let Some(action) = keys::map_key(&self.keymap, key) else {
            return;
        };

        // ── row inspector modal ──
        if self.show_row_inspector {
            match action {
                keys::AppAction::QuitOrBack
                | keys::AppAction::Enter
                | keys::AppAction::ToggleActionsMenu => {
                    self.close_row_inspector();
                }
                keys::AppAction::MoveUp => self.inspector_scroll.scroll_up(),
                keys::AppAction::MoveDown => self.inspector_scroll.scroll_down(),
                _ => {}
            }
            return;
        }

        // ── menú de acciones (modal) ──
        if self.show_actions_menu {
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
            return;
        }

        // ── acciones normales ──
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
            keys::AppAction::ToggleCurrentPanel => self.toggle_active_panel(),
            keys::AppAction::QuitOrBack => {
                if self.active_panel == PanelKind::Detail {
                    self.set_focus(PanelKind::Tables);
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
            keys::AppAction::MoveUp => self.move_selection(-1),
            keys::AppAction::MoveDown => self.move_selection(1),
            keys::AppAction::PrevPage => {
                if self.active_panel == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    self.current_page = self.current_page.saturating_sub(1);
                    self.refresh_preview_from_selected_object();
                }
            }
            keys::AppAction::NextPage => {
                if self.active_panel == PanelKind::Detail && self.detail_tab == DetailTab::Data {
                    self.current_page = self.current_page.saturating_add(1);
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
            let p = self.panel_mut(target);
            if up {
                p.scroll_offset.set(p.scroll_offset.get().saturating_sub(1));
                p.selected_idx = p.selected_idx.saturating_sub(1).min(items_len.saturating_sub(1));
            } else {
                // STOP: no pasar del último ítem
                let max_scroll = items_len.saturating_sub(1);
                p.scroll_offset.set(p.scroll_offset.get().saturating_add(1).min(max_scroll));
                p.selected_idx = (p.selected_idx + 1).min(items_len.saturating_sub(1));
            }
        }
    }

    /// Detecta qué tab del título de Detail fue clickeado
    /// Detección de click en pestañas del título de Detail.
    /// Formato: "[5] Datos - P1/11 | Esquema | SQL | Meta |"
    fn detect_detail_tab_click(&self, cursor_x: u16, rect: Rect) -> Option<DetailTab> {
        let available = self.available_detail_tabs();
        let num = PanelKind::Detail.number();
        // Saltar "[5]" (4 chars aprox) + 1 espacio
        let prefix = format!("[{num}]");
        #[allow(clippy::cast_possible_truncation)]
        let mut cursor = rect.x + prefix.len() as u16;

        for &tab in &available {
            let text_w = self.detail_tab_display_width(tab);
            if cursor_x >= cursor && cursor_x < cursor + text_w {
                return Some(tab);
            }
            // " | " = 3 chars
            cursor += text_w + 3;
        }
        None
    }

    /// Ancho en columnas del texto de un tab en el título.
    fn detail_tab_display_width(&self, tab: DetailTab) -> u16 {
        let label = tab.label();
        let inner = if tab == DetailTab::Data && self.total_rows > 0 {
            let total = self.total_rows.div_ceil(self.rows_per_page).max(1);
            format!("{label} - P{}/{}", self.current_page + 1, total)
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

    pub fn on_mouse_click(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width < 40 || height < 10 {
            return;
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
                // Click en título → focus + toggle (si no es Detail)
                if kind == PanelKind::Detail {
                    // Detectar click en tabs del título
                    if let Some(tab) = self.detect_detail_tab_click(x, rect) {
                        self.set_detail_tab(tab);
                    }
                    self.set_focus(PanelKind::Detail);
                } else {
                    self.set_focus(kind);
                }
                return;
            }

            // Click en contenido
            self.set_focus(kind);

            // Para Sources, manejar click en tabs (primeras 3 columnas = tabs)
            if kind == PanelKind::Sources && rel_y == 1 && rect.width >= 3 {
                let thirds = rect.width.max(3) / 3;
                let rel_x = x.saturating_sub(rect.x);
                if rel_x < thirds {
                    self.set_source_tab(SourceTab::All);
                    return;
                }
                if rel_x < thirds.saturating_mul(2) {
                    self.set_source_tab(SourceTab::Local);
                    return;
                }
                self.set_source_tab(SourceTab::Online);
                return;
            }

            // Click en un ítem de la lista
            if let Some(index) = list_index_from_click(rel_y, rect.height, 0) {
                let max_idx = self.items_len_for(kind).saturating_sub(1);
                let scroll = self.panel(kind).scroll_offset.get();
                let p = self.panel_mut(kind);
                p.selected_idx = (index + scroll).min(max_idx);

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
