use std::collections::HashSet;

use crossterm::event::KeyEvent;

use crate::{config, keys, query, sqlite, storage};

const LARGE_WIDTH: u16 = 120;
const MEDIUM_WIDTH: u16 = 80;
const KB_BYTES: u64 = 1024;
const MB_BYTES: u64 = KB_BYTES * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum FocusPanel {
    Sources,
    Objects,
    Preview,
}

impl FocusPanel {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Objects => "Objects",
            Self::Preview => "Detail",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Sources => Self::Objects,
            Self::Objects => Self::Preview,
            Self::Preview => Self::Sources,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::Sources => Self::Preview,
            Self::Objects => Self::Sources,
            Self::Preview => Self::Objects,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum LayoutMode {
    Large,
    Medium,
    Small,
}

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

impl LayoutMode {
    pub const fn from_width(width: u16) -> Self {
        if width >= LARGE_WIDTH {
            Self::Large
        } else if width >= MEDIUM_WIDTH {
            Self::Medium
        } else {
            Self::Small
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::Medium => "medium",
            Self::Small => "small",
        }
    }
}

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
            Self::Data => "Datos",
            Self::Schema => "Esquema",
            Self::Sql => "SQL",
            Self::Meta => "Meta",
        }
    }
}

enum EnterAction {
    Connect(String),
    UpdateStatus,
    None,
}

pub struct App {
    pub focus: FocusPanel,
    pub should_quit: bool,
    pub refresh_count: u32,
    pub source_idx: usize,
    pub object_idx: usize,
    pub preview_idx: usize,
    pub sources: Vec<String>,
    pub objects: Vec<String>,
    pub preview_rows: Vec<String>,
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
    pub source_tab: SourceTab,
    pub object_section: ObjectSection,
    pub detail_tab: DetailTab,
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub advanced: Vec<String>,
    pub show_actions_menu: bool,
    pub actions_menu_idx: usize,
}

impl App {
    const ACTION_ITEMS: [&'static str; 5] = [
        "Abrir sakila.db",
        "Guardar DB actual en favoritos",
        "Recargar config runtime",
        "Limpiar estado de query",
        "Cerrar menu",
    ];

    pub fn new() -> Self {
        let state = storage::AppState::load();
        let keymap = keys::Keymap::load();
        let ui_config = config::load_ui_config();
        let source_tab = SourceTab::All;

        Self {
            focus: FocusPanel::Sources,
            should_quit: false,
            refresh_count: 0,
            source_idx: 0,
            object_idx: 0,
            preview_idx: 0,
            sources: Self::build_sources(&state, source_tab),
            objects: vec![],
            preview_rows: vec!["Sin conexion SQLite".to_string()],
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
            source_tab,
            object_section: ObjectSection::Tables,
            detail_tab: DetailTab::Data,
            tables: Vec::new(),
            views: Vec::new(),
            advanced: Vec::new(),
            show_actions_menu: false,
            actions_menu_idx: 0,
        }
    }

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

    fn sync_objects_from_section(&mut self) {
        self.objects = match self.object_section {
            ObjectSection::Tables => self.tables.clone(),
            ObjectSection::Views => self.views.clone(),
            ObjectSection::Advanced => self.advanced.clone(),
        };

        if self.objects.is_empty() {
            self.objects.push("<sin objetos>".to_string());
        }

        self.object_idx = self.object_idx.min(self.objects.len().saturating_sub(1));
    }

    fn set_source_tab(&mut self, tab: SourceTab) {
        self.source_tab = tab;
        self.sources = Self::build_sources(&self.state, self.source_tab);
        self.source_idx = 0;
    }

    fn set_source_tab_from_click(&mut self, rel_x: u16, width: u16) {
        let thirds = width.max(3) / 3;
        let tab = if rel_x < thirds {
            SourceTab::All
        } else if rel_x < thirds.saturating_mul(2) {
            SourceTab::Local
        } else {
            SourceTab::Online
        };

        self.set_source_tab(tab);
    }

    fn select_source_index(&mut self, index: usize) {
        if self.sources.is_empty() {
            self.source_idx = 0;
            return;
        }

        self.source_idx = index.min(self.sources.len().saturating_sub(1));
    }

    fn set_object_section(&mut self, section: ObjectSection) {
        self.object_section = section;
        self.sync_objects_from_section();
        self.object_idx = 0;
        self.current_page = 0;
        self.refresh_preview_from_selected_object();
    }

    fn select_object_index(&mut self, index: usize) {
        if self.objects.is_empty() {
            self.object_idx = 0;
            return;
        }

        self.object_idx = index.min(self.objects.len().saturating_sub(1));
        self.current_page = 0;
        self.refresh_preview_from_selected_object();
    }

    fn set_detail_tab(&mut self, tab: DetailTab) {
        self.detail_tab = tab;
        self.preview_idx = 0;
        self.refresh_preview_from_selected_object();
    }

    fn select_preview_index(&mut self, index: usize) {
        if self.preview_rows.is_empty() {
            self.preview_idx = 0;
            return;
        }

        self.preview_idx = index.min(self.preview_rows.len().saturating_sub(1));
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let Some(action) = keys::map_key(&self.keymap, key) else {
            return;
        };

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

        match action {
            keys::AppAction::RunCountQuery => self.execute_count_query(),
            keys::AppAction::ClearQueryState => self.clear_query_state(),
            keys::AppAction::ReloadRuntimeConfig => self.reload_runtime_config(),
            keys::AppAction::ToggleActionsMenu => {
                self.show_actions_menu = true;
                self.actions_menu_idx = 0;
                self.status = "Menu de acciones abierto".to_string();
            }
            keys::AppAction::QuitOrBack => {
                if self.focus == FocusPanel::Preview {
                    self.focus = FocusPanel::Objects;
                } else {
                    self.should_quit = true;
                }
            }
            keys::AppAction::FocusPrev => self.focus = self.focus.prev(),
            keys::AppAction::FocusNext => self.focus = self.focus.next(),
            keys::AppAction::FocusSources => self.focus = FocusPanel::Sources,
            keys::AppAction::FocusObjects => self.focus = FocusPanel::Objects,
            keys::AppAction::FocusPreview => self.focus = FocusPanel::Preview,
            keys::AppAction::Refresh => {
                self.refresh_count = self.refresh_count.saturating_add(1);
                self.refresh_from_connection();
            }
            keys::AppAction::FavoriteCurrentDb => self.mark_current_db_as_favorite(),
            keys::AppAction::MoveUp => self.move_selection(-1),
            keys::AppAction::MoveDown => self.move_selection(1),
            keys::AppAction::PrevPage => {
                if self.focus == FocusPanel::Preview && self.detail_tab == DetailTab::Data {
                    self.current_page = self.current_page.saturating_sub(1);
                    self.refresh_preview_from_selected_object();
                }
            }
            keys::AppAction::NextPage => {
                if self.focus == FocusPanel::Preview && self.detail_tab == DetailTab::Data {
                    self.current_page = self.current_page.saturating_add(1);
                    self.refresh_preview_from_selected_object();
                }
            }
            keys::AppAction::Enter => self.handle_enter(),
            keys::AppAction::SourceTabRecents => self.set_source_tab(SourceTab::All),
            keys::AppAction::SourceTabFavorites => self.set_source_tab(SourceTab::Local),
            keys::AppAction::ObjectSectionTables => self.set_object_section(ObjectSection::Tables),
            keys::AppAction::ObjectSectionViews => self.set_object_section(ObjectSection::Views),
            keys::AppAction::ObjectSectionAdvanced => {
                self.set_object_section(ObjectSection::Advanced);
            }
            keys::AppAction::DetailTabPrev => {
                self.set_detail_tab(self.detail_tab.prev());
            }
            keys::AppAction::DetailTabNext => {
                self.set_detail_tab(self.detail_tab.next());
            }
            keys::AppAction::DetailTabData => self.set_detail_tab(DetailTab::Data),
            keys::AppAction::DetailTabSchema => self.set_detail_tab(DetailTab::Schema),
            keys::AppAction::DetailTabSql => self.set_detail_tab(DetailTab::Sql),
            keys::AppAction::DetailTabMeta => self.set_detail_tab(DetailTab::Meta),
            keys::AppAction::SourceTabNext => self.set_source_tab(self.source_tab.next()),
            keys::AppAction::SourceTabPrev => self.set_source_tab(self.source_tab.prev()),
        }
    }

    pub fn on_scroll(&mut self, up: bool) {
        if self.show_actions_menu {
            if up {
                self.actions_menu_idx = self.actions_menu_idx.saturating_sub(1);
            } else {
                let last = Self::ACTION_ITEMS.len().saturating_sub(1);
                self.actions_menu_idx = (self.actions_menu_idx + 1).min(last);
            }
            return;
        }

        if up {
            self.move_selection(-1);
        } else {
            self.move_selection(1);
        }
    }

    pub fn on_mouse_click(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width < 40 || height < 10 {
            return;
        }

        let compact_height = height < 14;
        let header_height = if compact_height { 1 } else { 2 };
        let footer_height = 1;

        if y < header_height || y >= height.saturating_sub(footer_height) {
            return;
        }

        let content_top = header_height;
        let content_height = height.saturating_sub(header_height + footer_height);

        if LayoutMode::from_width(width) == LayoutMode::Small {
            return;
        }

        let left_width = width.saturating_mul(33) / 100;

        if x < left_width {
            let rel_y = y.saturating_sub(content_top);
            let rel_x = x;
            let h0 = content_height.saturating_mul(30) / 100;
            let h1 = content_height.saturating_mul(30) / 100;
            let h2 = content_height.saturating_mul(20) / 100;

            if rel_y < h0 {
                self.focus = FocusPanel::Sources;
                if rel_y == 0 {
                    self.set_source_tab_from_click(rel_x, left_width);
                    return;
                }
                if let Some(index) = list_index_from_click(rel_y, h0, 0) {
                    self.select_source_index(index);
                }
                return;
            }

            if rel_y < h0 + h1 {
                self.focus = FocusPanel::Objects;
                self.set_object_section(ObjectSection::Tables);
                if let Some(index) = list_index_from_click(rel_y - h0, h1, 0) {
                    self.select_object_index(index);
                }
                return;
            }

            if rel_y < h0 + h1 + h2 {
                self.focus = FocusPanel::Objects;
                self.set_object_section(ObjectSection::Views);
                if let Some(index) = list_index_from_click(rel_y - (h0 + h1), h2, 0) {
                    self.select_object_index(index);
                }
                return;
            }

            let h3 = content_height.saturating_sub(h0 + h1 + h2);
            self.focus = FocusPanel::Objects;
            self.set_object_section(ObjectSection::Advanced);
            if let Some(index) = list_index_from_click(rel_y - (h0 + h1 + h2), h3, 0) {
                self.select_object_index(index);
            }
            return;
        }

        self.focus = FocusPanel::Preview;
        let right_x = x.saturating_sub(left_width);
        let right_width = width.saturating_sub(left_width);
        let rel_y = y.saturating_sub(content_top);

        if rel_y < 2 {
            let slot = if right_width == 0 {
                0
            } else {
                usize::from(right_x)
                    .saturating_mul(4)
                    .saturating_div(usize::from(right_width.max(1)))
                    .min(3)
            };
            match slot {
                0 => self.set_detail_tab(DetailTab::Data),
                1 => self.set_detail_tab(DetailTab::Schema),
                2 => self.set_detail_tab(DetailTab::Sql),
                _ => self.set_detail_tab(DetailTab::Meta),
            }
            return;
        }

        let content_h = content_height.saturating_sub(2);
        if let Some(index) = list_index_from_click(rel_y - 2, content_h, 0) {
            self.select_preview_index(index);
        }
    }

    pub fn selected_source(&self) -> &str {
        self.sources.get(self.source_idx).map_or("-", String::as_str)
    }

    pub fn selected_object(&self) -> &str {
        self.objects.get(self.object_idx).map_or("-", String::as_str)
    }

    pub const fn source_tab_label(&self) -> &'static str {
        self.source_tab.label()
    }

    pub const fn object_section_label(&self) -> &'static str {
        self.object_section.label()
    }

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

    fn move_selection(&mut self, step: isize) {
        match self.focus {
            FocusPanel::Sources => {
                Self::shift_index(&mut self.source_idx, self.sources.len(), step);
            }
            FocusPanel::Objects => {
                Self::shift_index(&mut self.object_idx, self.objects.len(), step);
                self.current_page = 0;
                self.refresh_preview_from_selected_object();
            }
            FocusPanel::Preview => {
                Self::shift_index(&mut self.preview_idx, self.preview_rows.len(), step);
            }
        }
    }

    fn shift_index(current: &mut usize, len: usize, step: isize) {
        if len == 0 {
            *current = 0;
            return;
        }

        let last = len.saturating_sub(1);
        let next = current.saturating_add_signed(step);
        *current = next.min(last);
    }

    fn handle_enter(&mut self) {
        if self.focus != FocusPanel::Sources {
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

    fn clear_query_state(&mut self) {
        self.query_state = query::QueryState::Idle;
        self.query_results.clear();
        self.status = "Query limpia".to_string();
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

        if self.source_idx >= self.sources.len() {
            self.source_idx = self.sources.len().saturating_sub(1);
        }

        if self.object_idx >= self.objects.len() {
            self.object_idx = self.objects.len().saturating_sub(1);
        }

        if self.preview_idx >= self.preview_rows.len() {
            self.preview_idx = self.preview_rows.len().saturating_sub(1);
        }

        if self.db_path.is_some() {
            self.refresh_preview_from_selected_object();
        }

        self.status =
            format!("Config recargada: keys + estado + ui (rows_per_page={})", self.rows_per_page);
    }

    fn connect_sqlite(&mut self, path: &str) {
        let tables = sqlite::list_objects_by_type(path, "table");
        let views = sqlite::list_objects_by_type(path, "view");
        let advanced = sqlite::list_advanced_objects(path);

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
                self.sync_objects_from_section();
                self.object_idx = 0;
                self.current_page = 0;
                self.detail_tab = DetailTab::Data;
                self.preview_idx = 0;
                self.refresh_preview_from_selected_object();
                self.status = format!("Conectado en modo read-only: {path}");
                self.focus = FocusPanel::Objects;
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

    fn selected_object_name(&self) -> String {
        let raw = self.selected_object();
        if self.object_section == ObjectSection::Advanced
            && let Some((_, name)) = raw.split_once(':')
        {
            return name.to_string();
        }

        raw.to_string()
    }

    fn refresh_preview_from_selected_object(&mut self) {
        let Some(path) = self.db_path.as_deref() else {
            return;
        };

        let object_name = self.selected_object_name();
        if object_name.is_empty() || object_name == "-" || object_name == "<sin objetos>" {
            self.preview_rows = vec!["Sin objeto seleccionado".to_string()];
            self.total_rows = 0;
            return;
        }

        match self.detail_tab {
            DetailTab::Data => {
                if self.object_section == ObjectSection::Advanced {
                    self.preview_rows =
                        vec!["No hay preview de datos para indices/triggers".to_string()];
                    self.total_rows = 0;
                    self.preview_idx = 0;
                    return;
                }

                match sqlite::table_row_count(path, &object_name) {
                    Ok(count) => {
                        self.total_rows = count;
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error contando filas: {err}")];
                        self.total_rows = 0;
                        self.preview_idx = 0;
                        return;
                    }
                }

                let offset = self.current_page.saturating_mul(self.rows_per_page);
                match sqlite::table_rows(path, &object_name, self.rows_per_page, offset) {
                    Ok(rows) => {
                        self.preview_rows =
                            if rows.is_empty() { vec!["<sin datos>".to_string()] } else { rows };
                        self.preview_idx = 0;
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error obteniendo filas: {err}")];
                        self.preview_idx = 0;
                    }
                }
            }
            DetailTab::Schema => {
                if self.object_section == ObjectSection::Advanced {
                    self.preview_rows =
                        vec!["Sin esquema tabular para este tipo de objeto".to_string()];
                    self.total_rows = 0;
                    self.preview_idx = 0;
                    return;
                }

                match sqlite::table_columns(path, &object_name) {
                    Ok(columns) => {
                        self.preview_rows = if columns.is_empty() {
                            vec!["Sin columnas visibles".to_string()]
                        } else {
                            columns
                        };
                        self.total_rows = 0;
                        self.preview_idx = 0;
                    }
                    Err(err) => {
                        self.preview_rows = vec![format!("Error schema: {err}")];
                        self.total_rows = 0;
                        self.preview_idx = 0;
                    }
                }
            }
            DetailTab::Sql => match sqlite::object_sql(path, &object_name) {
                Ok(sql) => {
                    self.preview_rows = sql.lines().map(ToString::to_string).collect::<Vec<_>>();
                    if self.preview_rows.is_empty() {
                        self.preview_rows = vec!["-- SQL vacio --".to_string()];
                    }
                    self.total_rows = 0;
                    self.preview_idx = 0;
                }
                Err(err) => {
                    self.preview_rows = vec![format!("Error SQL: {err}")];
                    self.total_rows = 0;
                    self.preview_idx = 0;
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
                self.preview_idx = 0;
            }
        }
    }

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
        if object.is_empty() || object == "-" || object == "<sin objetos>" {
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
}
