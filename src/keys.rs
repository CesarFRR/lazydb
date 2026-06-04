use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppAction {
    RunCountQuery,
    ClearQueryState,
    ReloadRuntimeConfig,
    QuitOrBack,
    /// Navegar al panel anterior (cicla los 5 paneles)
    FocusPrev,
    /// Navegar al panel siguiente (cicla los 5 paneles)
    FocusNext,
    /// Navegar entre paneles izquierdos (←, cíclico Sources↔Advanced)
    SidebarFocusPrev,
    /// Navegar entre paneles izquierdos (→, cíclico Sources↔Tables)
    SidebarFocusNext,
    /// Ir directamente a Fuentes
    FocusSources,
    /// Ir directamente a Tablas
    FocusTables,
    /// Ir directamente a Vistas
    FocusViews,
    /// Ir directamente a Avanzado
    FocusAdvanced,
    /// Ir directamente a Detalle
    FocusDetail,
    /// (obsoleto, redirige a `FocusTables` para compatibilidad)
    FocusObjects,
    /// (obsoleto, redirige a `FocusDetail` para compatibilidad)
    FocusPreview,
    /// Toggle expandir/colapsar el panel actual
    ToggleCurrentPanel,
    /// Saltar al panel Detalle sin colapsar el panel sidebar actual
    JumpToDetail,
    Refresh,
    FavoriteCurrentDb,
    MoveUp,
    MoveDown,
    PrevPage,
    NextPage,
    Enter,
    SourceTabRecents,
    SourceTabFavorites,
    ObjectSectionTables,
    ObjectSectionViews,
    ObjectSectionAdvanced,
    DetailTabPrev,
    DetailTabNext,
    DetailTabData,
    DetailTabSchema,
    DetailTabSql,
    DetailTabMeta,
    SourceTabNext,
    SourceTabPrev,
    ToggleActionsMenu,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: HashMap<String, AppAction>,
}

impl Keymap {
    pub fn load() -> Self {
        let mut keymap = Self::default();

        let path = config_file_path();
        let Ok(content) = fs::read_to_string(path) else {
            return keymap;
        };

        let Ok(parsed) = content.parse::<toml::Value>() else {
            return keymap;
        };

        let Some(table) = parsed.get("keys").and_then(toml::Value::as_table) else {
            return keymap;
        };

        for (action_name, value) in table {
            let Some(action) = action_from_name(action_name) else {
                continue;
            };

            if let Some(token) = value.as_str() {
                keymap.set_binding(token, action);
                continue;
            }

            if let Some(tokens) = value.as_array() {
                for token in tokens {
                    if let Some(token_str) = token.as_str() {
                        keymap.set_binding(token_str, action);
                    }
                }
            }
        }

        keymap
    }

    fn set_binding(&mut self, token: &str, action: AppAction) {
        self.bindings.retain(|_, existing| *existing != action);
        self.bindings.insert(normalize_token(token), action);
    }
}

impl Default for Keymap {
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // ── queries ──
        bindings.insert("ctrl+q".to_string(), AppAction::RunCountQuery);
        bindings.insert("ctrl+l".to_string(), AppAction::ClearQueryState);
        bindings.insert("ctrl+r".to_string(), AppAction::ReloadRuntimeConfig);

        // ── navegación global ──
        bindings.insert("esc".to_string(), AppAction::QuitOrBack);
        bindings.insert("q".to_string(), AppAction::QuitOrBack);
        bindings.insert("r".to_string(), AppAction::Refresh);
        bindings.insert("f".to_string(), AppAction::FavoriteCurrentDb);

        // ── foco entre paneles (Tab / Shift+Tab) ──
        bindings.insert("tab".to_string(), AppAction::FocusNext);
        bindings.insert("shift+tab".to_string(), AppAction::FocusPrev);

        // ── ir a panel específico (1-5) ──
        bindings.insert("1".to_string(), AppAction::FocusSources);
        bindings.insert("2".to_string(), AppAction::FocusTables);
        bindings.insert("3".to_string(), AppAction::FocusViews);
        bindings.insert("4".to_string(), AppAction::FocusAdvanced);
        bindings.insert("5".to_string(), AppAction::FocusDetail);

        // ── toggle panel ──
        bindings.insert(" ".to_string(), AppAction::ToggleCurrentPanel);

        // ── mover selección ──
        bindings.insert("up".to_string(), AppAction::MoveUp);
        bindings.insert("k".to_string(), AppAction::MoveUp);
        bindings.insert("down".to_string(), AppAction::MoveDown);
        bindings.insert("j".to_string(), AppAction::MoveDown);

        // ── mover foco entre sidebar (← →) ──
        bindings.insert("left".to_string(), AppAction::SidebarFocusPrev);
        bindings.insert("right".to_string(), AppAction::SidebarFocusNext);

        // ── tabs de detalle ──
        bindings.insert("[".to_string(), AppAction::DetailTabPrev);
        bindings.insert("]".to_string(), AppAction::DetailTabNext);

        // ── paginación ──
        bindings.insert("pgup".to_string(), AppAction::PrevPage);
        bindings.insert("pgdn".to_string(), AppAction::NextPage);

        // ── acciones ──
        bindings.insert("enter".to_string(), AppAction::JumpToDetail);
        bindings.insert("x".to_string(), AppAction::ToggleActionsMenu);
        bindings.insert("b".to_string(), AppAction::ToggleActionsMenu);

        Self { bindings }
    }
}

pub fn map_key(keymap: &Keymap, key: KeyEvent) -> Option<AppAction> {
    let token = token_from_key(key)?;
    keymap.bindings.get(&token).copied()
}

fn token_from_key(key: KeyEvent) -> Option<String> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(ch) = key.code
    {
        return Some(format!("ctrl+{}", ch.to_ascii_lowercase()));
    }

    match key.code {
        KeyCode::Esc => Some("esc".to_string()),
        KeyCode::Tab => Some("tab".to_string()),
        KeyCode::BackTab => Some("shift+tab".to_string()),
        KeyCode::Enter => Some("enter".to_string()),
        KeyCode::Up => Some("up".to_string()),
        KeyCode::Down => Some("down".to_string()),
        KeyCode::Left => Some("left".to_string()),
        KeyCode::Right => Some("right".to_string()),
        KeyCode::PageUp => Some("pgup".to_string()),
        KeyCode::PageDown => Some("pgdn".to_string()),
        KeyCode::Char(ch) => Some(ch.to_ascii_lowercase().to_string()),
        _ => None,
    }
}

fn action_from_name(name: &str) -> Option<AppAction> {
    match name {
        "run_count_query" => Some(AppAction::RunCountQuery),
        "clear_query_state" => Some(AppAction::ClearQueryState),
        "reload_runtime_config" => Some(AppAction::ReloadRuntimeConfig),
        "quit_or_back" => Some(AppAction::QuitOrBack),
        "focus_prev" => Some(AppAction::FocusPrev),
        "focus_next" => Some(AppAction::FocusNext),
        "sidebar_focus_prev" => Some(AppAction::SidebarFocusPrev),
        "sidebar_focus_next" => Some(AppAction::SidebarFocusNext),
        "focus_sources" => Some(AppAction::FocusSources),
        "focus_tables" => Some(AppAction::FocusTables),
        "focus_views" => Some(AppAction::FocusViews),
        "focus_advanced" => Some(AppAction::FocusAdvanced),
        "focus_detail" => Some(AppAction::FocusDetail),
        "focus_objects" => Some(AppAction::FocusObjects),
        "focus_preview" => Some(AppAction::FocusPreview),
        "toggle_current_panel" => Some(AppAction::ToggleCurrentPanel),
        "jump_to_detail" => Some(AppAction::JumpToDetail),
        "refresh" => Some(AppAction::Refresh),
        "favorite_current_db" => Some(AppAction::FavoriteCurrentDb),
        "move_up" => Some(AppAction::MoveUp),
        "move_down" => Some(AppAction::MoveDown),
        "prev_page" => Some(AppAction::PrevPage),
        "next_page" => Some(AppAction::NextPage),
        "enter" => Some(AppAction::Enter),
        "source_tab_recents" => Some(AppAction::SourceTabRecents),
        "source_tab_favorites" => Some(AppAction::SourceTabFavorites),
        "object_section_tables" => Some(AppAction::ObjectSectionTables),
        "object_section_views" => Some(AppAction::ObjectSectionViews),
        "object_section_advanced" => Some(AppAction::ObjectSectionAdvanced),
        "detail_tab_prev" => Some(AppAction::DetailTabPrev),
        "detail_tab_next" => Some(AppAction::DetailTabNext),
        "detail_tab_data" => Some(AppAction::DetailTabData),
        "detail_tab_schema" => Some(AppAction::DetailTabSchema),
        "detail_tab_sql" => Some(AppAction::DetailTabSql),
        "detail_tab_meta" => Some(AppAction::DetailTabMeta),
        "source_tab_next" => Some(AppAction::SourceTabNext),
        "source_tab_prev" => Some(AppAction::SourceTabPrev),
        "toggle_actions_menu" => Some(AppAction::ToggleActionsMenu),
        _ => None,
    }
}

fn normalize_token(token: &str) -> String {
    token.trim().to_ascii_lowercase()
}

fn config_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("lazydb").join("config.toml")
}
