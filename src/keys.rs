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
    FocusPrev,
    FocusNext,
    FocusSources,
    FocusObjects,
    FocusPreview,
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

        bindings.insert("ctrl+q".to_string(), AppAction::RunCountQuery);
        bindings.insert("ctrl+l".to_string(), AppAction::ClearQueryState);
        bindings.insert("ctrl+r".to_string(), AppAction::ReloadRuntimeConfig);
        bindings.insert("esc".to_string(), AppAction::QuitOrBack);
        bindings.insert("q".to_string(), AppAction::QuitOrBack);
        bindings.insert("tab".to_string(), AppAction::SourceTabNext);
        bindings.insert("shift+tab".to_string(), AppAction::SourceTabPrev);
        bindings.insert("r".to_string(), AppAction::Refresh);
        bindings.insert("f".to_string(), AppAction::FavoriteCurrentDb);
        bindings.insert("up".to_string(), AppAction::MoveUp);
        bindings.insert("k".to_string(), AppAction::MoveUp);
        bindings.insert("down".to_string(), AppAction::MoveDown);
        bindings.insert("j".to_string(), AppAction::MoveDown);
        bindings.insert("left".to_string(), AppAction::DetailTabPrev);
        bindings.insert("right".to_string(), AppAction::DetailTabNext);
        bindings.insert("pgup".to_string(), AppAction::PrevPage);
        bindings.insert("pgdn".to_string(), AppAction::NextPage);
        bindings.insert("enter".to_string(), AppAction::Enter);
        bindings.insert("x".to_string(), AppAction::ToggleActionsMenu);
        bindings.insert("b".to_string(), AppAction::ToggleActionsMenu);
        bindings.insert("[".to_string(), AppAction::DetailTabPrev);
        bindings.insert("]".to_string(), AppAction::DetailTabNext);

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
        "focus_sources" => Some(AppAction::FocusSources),
        "focus_objects" => Some(AppAction::FocusObjects),
        "focus_preview" => Some(AppAction::FocusPreview),
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
