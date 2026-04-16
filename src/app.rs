use crossterm::event::KeyCode;

use crate::sqlite;

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
}

impl App {
    pub fn new() -> Self {
        Self {
            focus: FocusPanel::Sources,
            should_quit: false,
            refresh_count: 0,
            source_idx: 0,
            object_idx: 0,
            preview_idx: 0,
            sources: vec![
                "Recientes".to_string(),
                "Buscar archivo .db".to_string(),
                "Abrir sakila.db".to_string(),
                "Favoritos".to_string(),
            ],
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
        }
    }

    pub fn on_key(&mut self, code: KeyCode) {
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

        match self.selected_source() {
            "Abrir sakila.db" => {
                self.connect_sqlite("sakila.db");
            }
            "Recientes" => {
                self.status = "Recientes aun no implementado".to_string();
            }
            "Favoritos" => {
                self.status = "Favoritos aun no implementado".to_string();
            }
            "Buscar archivo .db" => {
                self.status = "Buscador de archivos .db llega en la siguiente seccion".to_string();
            }
            _ => {}
        }
    }

    fn connect_sqlite(&mut self, path: &str) {
        match sqlite::list_objects(path) {
            Ok(mut objects) => {
                if objects.is_empty() {
                    objects.push("<sin objetos>".to_string());
                }

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
}
