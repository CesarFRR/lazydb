//! Handlers de TECLADO de `App` (extraído del monolito: la subcarpeta
//! `controller/` divide el event loop por responsabilidad — teclado aquí,
//! mouse en `mouse.rs`, el resto en `mod.rs`).

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, ConnField, DetailTab, InputMode, PanelKind, SourceTab};
use crate::app::sources::{is_source_section, strip_source_marks};
use crate::keys;
use crate::query;

impl App {
    /// Conecta a la fuente seleccionada en el panel Sources
    pub(super) fn connect_selected_source(&mut self) {
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
        } else if self.connection.password_prompt.is_some() {
            self.connection.password_prompt = None;
            self.status = String::new();
        } else if self.connection.db_picker.is_some() {
            self.connection.db_picker = None;
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
        if self.connection.connection_form.is_some() && self.db_path.is_none() {
            if let Some(form) = self.connection.connection_form.as_mut() {
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
            if let Some(form) = self.connection.connection_form.as_ref() {
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
        if self.query.query_input.is_some() {
            if let Some(state) = self.query.query_input.as_mut() {
                state.buffer.push_str(&clean);
            }
            return;
        }

        // Prompt de contraseña
        if self.connection.password_prompt.is_some() {
            if let Some(state) = self.connection.password_prompt.as_mut() {
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
        if self.connection.password_prompt.is_some() {
            self.handle_password_prompt_key(key);
            return;
        }
        if self.connection.db_picker.is_some() {
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

        // ── pestaña Query: el foco en Detail + tab Query escribe en el
        // buffer compartido (sin abrir el modal). Enter ejecuta, Ctrl+U
        // limpia, ↑/↓ historial, j/k scroll de resultados. ──
        if self.db_path.is_some()
            && self.active_panel == PanelKind::Detail
            && self.data_view.detail_tab == DetailTab::Query
        {
            self.handle_query_tab_key(key);
            return;
        }

        // ── input SQL (modal `:` — captura TODO mientras está abierto,
        // incluidos chars no mapeados a ninguna acción) ──
        if self.query.query_input.is_some() {
            self.handle_query_input_key(key);
            return;
        }

        // ── pick de base de datos (modal de servidor detectado) ──
        if self.connection.db_picker.is_some() {
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
                self.query.query_input = Some(query::QueryInputState::default());
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
                if self.active_panel == PanelKind::Detail
                    && self.data_view.detail_tab == DetailTab::Data
                {
                    self.move_selection_by_page(false);
                }
            }
            keys::AppAction::NextPage => {
                if self.active_panel == PanelKind::Detail
                    && self.data_view.detail_tab == DetailTab::Data
                {
                    self.move_selection_by_page(true);
                }
            }
            keys::AppAction::JumpToDetail => self.jump_to_detail(),
            keys::AppAction::Enter => {
                // Pestaña Query: Enter ejecuta la query del buffer (sin abrir
                // el modal `:`). En cualquier otro panel: comportamiento normal.
                if self.data_view.detail_tab == DetailTab::Query
                    && self.active_panel == PanelKind::Detail
                {
                    self.execute_query_tab();
                } else {
                    self.handle_enter();
                }
            }
            keys::AppAction::SourceTabRecents => self.set_source_tab(SourceTab::All),
            keys::AppAction::SourceTabFavorites => self.set_source_tab(SourceTab::Local),
            keys::AppAction::SourceTabNext => {
                self.set_source_tab(self.sources_state.source_tab.next());
            }
            keys::AppAction::SourceTabPrev => {
                self.set_source_tab(self.sources_state.source_tab.prev());
            }
            keys::AppAction::DetailTabPrev => self.set_detail_tab(self.data_view.detail_tab.prev()),
            keys::AppAction::DetailTabNext => self.set_detail_tab(self.data_view.detail_tab.next()),
            keys::AppAction::DetailTabData => self.set_detail_tab(DetailTab::Data),
            keys::AppAction::DetailTabSchema => self.set_detail_tab(DetailTab::Schema),
            keys::AppAction::DetailTabQuery => self.set_detail_tab(DetailTab::Query),
            keys::AppAction::DetailTabMeta => self.set_detail_tab(DetailTab::Meta),
            keys::AppAction::ObjectSectionAdvanced => {
                self.set_focus(PanelKind::Advanced);
            }
        }
    }
}
