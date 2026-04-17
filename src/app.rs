use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{query, sqlite, storage};

const LARGE_WIDTH: u16 = 120;
const MEDIUM_WIDTH: u16 = 80;

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
            Self::Preview => "Preview",
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

#[derive(Clone, Copy)]
pub enum LayoutMode {
    Large,
    Medium,
    Small,
}

enum EnterAction {
    Connect(String),
    UpdateStatus,
    None,
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
    pub status: String,
    pub current_page: u32,
    pub rows_per_page: u32,
    pub total_rows: u32,
    pub query_state: query::QueryState,
    pub query_results: Vec<String>,
    pub state: storage::AppState,
}

impl App {
    pub fn new() -> Self {
        let state = storage::AppState::load();

        // Construir sources con recents
        let mut sources = vec![
            "Buscar archivo .db".to_string(),
            "Abrir sakila.db".to_string(),
            "Favoritos".to_string(),
        ];

        // Insertar recents al inicio si existen
        if !state.recents.is_empty() {
            sources.insert(0, "─ Recientes ─".to_string());
            for recent in state.recents.iter().rev() {
                sources.insert(1, recent.clone());
            }
        }

        Self {
            focus: FocusPanel::Sources,
            should_quit: false,
            refresh_count: 0,
            source_idx: 0,
            object_idx: 0,
            preview_idx: 0,
            sources,
            objects: vec![
                "actor".to_string(),
                "address".to_string(),
                "category".to_string(),
                "city".to_string(),
                "country".to_string(),
                "customer".to_string(),
                "film".to_string(),
                "film_actor".to_string(),
                "inventory".to_string(),
                "payment".to_string(),
                "rental".to_string(),
                "staff".to_string(),
            ],
            preview_rows: vec![
                "id | first_name | last_name".to_string(),
                "1  | PENELOPE   | GUINESS".to_string(),
                "2  | NICK       | WAHLBERG".to_string(),
                "3  | ED         | CHASE".to_string(),
                "4  | JENNIFER   | DAVIS".to_string(),
                "5  | JOHNNY     | LOLLOBRIGIDA".to_string(),
            ],
            db_path: None,
            status: "Sin conexion SQLite".to_string(),
            current_page: 0,
            rows_per_page: 10,
            total_rows: 0,
            query_state: query::QueryState::Idle,
            query_results: Vec::new(),
            state,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let code = key.code;
        let is_ctrl_q = key.modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('q');

        if is_ctrl_q {
            self.execute_count_query();
            return;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                self.focus = self.focus.prev();
                if self.focus == FocusPanel::Preview {
                    self.current_page = 0;
                }
            }
            KeyCode::Tab | KeyCode::Char('l') => {
                self.focus = self.focus.next();
                if self.focus == FocusPanel::Preview {
                    self.current_page = 0;
                }
            }
            KeyCode::Char('1') => {
                self.focus = FocusPanel::Sources;
            }
            KeyCode::Char('2') => {
                self.focus = FocusPanel::Objects;
            }
            KeyCode::Char('3') => {
                self.focus = FocusPanel::Preview;
                self.current_page = 0;
            }
            KeyCode::Char('r') => {
                self.refresh_count = self.refresh_count.saturating_add(1);
                self.refresh_from_connection();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
            }
            KeyCode::PageUp => {
                if self.focus == FocusPanel::Preview {
                    self.current_page = self.current_page.saturating_sub(1);
                    self.refresh_preview_from_selected_object();
                }
            }
            KeyCode::PageDown => {
                if self.focus == FocusPanel::Preview {
                    self.current_page = self.current_page.saturating_add(1);
                    self.refresh_preview_from_selected_object();
                }
            }
            KeyCode::Enter => {
                self.handle_enter();
            }
            _ => {}
        }
    }

    pub fn selected_source(&self) -> &str {
        self.sources.get(self.source_idx).map_or("-", String::as_str)
    }

    pub fn selected_object(&self) -> &str {
        self.objects.get(self.object_idx).map_or("-", String::as_str)
    }

    pub fn selected_preview_row(&self) -> &str {
        self.preview_rows.get(self.preview_idx).map_or("-", String::as_str)
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

        // Ignorar líneas separadoras
        if selected == "─ Recientes ─" {
            return;
        }

        // 1. Decidimos qué hacer (Préstamo inmutable)
        let action = match selected.as_str() {
            "Abrir sakila.db" => EnterAction::Connect("sakila.db".to_string()),
            "Buscar archivo .db" => EnterAction::UpdateStatus,
            s if s.starts_with('/')
                || std::path::Path::new(s)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("db")) =>
            {
                EnterAction::Connect(s.to_string())
            }
            _ => EnterAction::None, // <-- Esto asegura que siempre haya una respuesta
        };

        // 2. Ejecutamos la acción (Préstamo mutable)
        match action {
            EnterAction::Connect(path) => self.connect_sqlite(&path),
            EnterAction::UpdateStatus => self.status = "Buscador de archivos...".to_string(),
            EnterAction::None => {}
        }
    }

    fn connect_sqlite(&mut self, path: &str) {
        match sqlite::list_objects(path) {
            Ok(mut objects) => {
                if objects.is_empty() {
                    objects.push("<sin objetos>".to_string());
                }

                // Agregar a recents y guardar
                let path_str = path.to_string();
                self.state.add_recent(path_str);
                let _ = self.state.save();

                self.db_path = Some(path.to_string());
                self.objects = objects;
                self.object_idx = 0;
                self.preview_idx = 0;
                self.refresh_preview_from_selected_object();
                self.status = format!("Conectado en modo read-only: {path}");
                self.focus = FocusPanel::Objects;
            }
            Err(err) => {
                self.status = format!("Error al abrir {path}: {err}");
            }
        }
    }

    fn refresh_from_connection(&mut self) {
        if let Some(path) = self.db_path.clone() {
            self.connect_sqlite(&path);
        }
    }

    fn refresh_preview_from_selected_object(&mut self) {
        let Some(path) = self.db_path.as_deref() else {
            return;
        };

        let object = self.selected_object().to_string();
        if object.is_empty() || object == "-" || object == "<sin objetos>" {
            return;
        }

        // Get total row count
        match sqlite::table_row_count(path, &object) {
            Ok(count) => {
                self.total_rows = count;
            }
            Err(err) => {
                self.preview_rows = vec![format!("Error contando filas: {err}")];
                self.total_rows = 0;
                return;
            }
        }

        // Calculate offset from current page
        let offset = self.current_page.saturating_mul(self.rows_per_page);

        // Fetch rows for this page
        match sqlite::table_rows(path, &object, self.rows_per_page, offset) {
            Ok(rows) => {
                if rows.is_empty() {
                    self.preview_rows = vec!["<sin datos>".to_string()];
                } else {
                    self.preview_rows = rows;
                }
                self.preview_idx = 0;
            }
            Err(err) => {
                self.preview_rows = vec![format!("Error obteniendo filas: {err}")];
                self.preview_idx = 0;
            }
        }
    }

    fn execute_count_query(&mut self) {
        let Some(path) = self.db_path.as_deref() else {
            self.status = "No hay DB conectada".to_string();
            return;
        };

        let object = self.selected_object().to_string();
        if object.is_empty() || object == "-" || object == "<sin objetos>" {
            self.status = "Selecciona una tabla primero".to_string();
            return;
        }

        // Ejecutar COUNT(*) en la tabla
        let sql = format!("SELECT COUNT(*) FROM \"{}\";", object.replace('"', "\"\""));

        self.query_state = query::QueryState::Running;
        self.status = "Ejecutando query...".to_string();

        // Nota: Por ahora es bloqueante. En Sección 5 refactorizar a async con canales.
        // Para hacerlo async sin bloquear la UI necesitaremos tokio::spawn + mpsc::channel
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
