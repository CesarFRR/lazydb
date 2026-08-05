use std::collections::HashMap;
use std::fs;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AppAction {
    RunCountQuery,
    ClearQueryState,
    ReloadRuntimeConfig,
    /// Abrir el input SQL (`:` estilo vim; historial con ↑/↓)
    OpenQueryInput,
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
    /// Favoritear la DB actualmente conectada (sin binding por defecto)
    FavoriteCurrentDb,
    /// Toggle favorito del item bajo el cursor en Fuentes (o la DB conectada)
    ToggleFavoriteSource,
    /// Olvidar la fuente bajo el cursor (quitar de recientes/favoritos)
    ForgetSource,
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
    /// Copiar ítem seleccionado al portapapeles
    Yank,
    /// Exportar tabla actual a CSV
    ExportCsv,
    /// Alternar pares ↔ JSON en el modal de detalles (solo `NoSQL`)
    ToggleInspectorJson,
    /// Iniciar filtro de búsqueda /
    StartFilter,
    /// Scroll horizontal de columnas (shift+h)
    HScrollLeft,
    /// Scroll horizontal de columnas (shift+l)
    HScrollRight,
    /// Ayuda de teclas (?)
    ToggleHelp,
}

/// Grupos de la ayuda de teclas (patrón lazygit §5.3: la ayuda se
/// autogenera desde los bindings REALES, no desde un doc hardcodeado).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyGroup {
    /// Moverse y salir
    Navigation,
    /// Foco entre paneles
    Focus,
    /// Gestión de fuentes (favoritos, olvidar, tabs)
    Sources,
    /// Pestañas de Detalle y secciones
    Tabs,
    /// Queries y scroll de datos
    Data,
    /// Acciones (menú, yank, CSV, filtro)
    Actions,
}

impl KeyGroup {
    /// Título de la sección en la ayuda.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navegación",
            Self::Focus => "Foco y paneles",
            Self::Sources => "Fuentes",
            Self::Tabs => "Pestañas y secciones",
            Self::Data => "Datos y queries",
            Self::Actions => "Acciones",
        }
    }
}

impl AppAction {
    /// Grupo de la ayuda al que pertenece la acción.
    pub const fn group(self) -> KeyGroup {
        use AppAction::{
            ClearQueryState, DetailTabData, DetailTabMeta, DetailTabNext, DetailTabPrev,
            DetailTabSchema, DetailTabSql, Enter, ExportCsv, FavoriteCurrentDb, FocusAdvanced,
            FocusDetail, FocusNext, FocusObjects, FocusPrev, FocusPreview, FocusSources,
            FocusTables, FocusViews, ForgetSource, HScrollLeft, HScrollRight, JumpToDetail,
            MoveDown, MoveUp, NextPage, ObjectSectionAdvanced, ObjectSectionTables,
            ObjectSectionViews, OpenQueryInput, PrevPage, QuitOrBack, Refresh, ReloadRuntimeConfig,
            RunCountQuery, SidebarFocusNext, SidebarFocusPrev, SourceTabFavorites, SourceTabNext,
            SourceTabPrev, SourceTabRecents, StartFilter, ToggleActionsMenu, ToggleCurrentPanel,
            ToggleFavoriteSource, ToggleHelp, ToggleInspectorJson, Yank,
        };
        match self {
            MoveUp | MoveDown | PrevPage | NextPage | QuitOrBack | Refresh => KeyGroup::Navigation,
            FocusPrev | FocusNext | SidebarFocusPrev | SidebarFocusNext | FocusSources
            | FocusTables | FocusViews | FocusAdvanced | FocusDetail | FocusObjects
            | FocusPreview | ToggleCurrentPanel | JumpToDetail => KeyGroup::Focus,
            FavoriteCurrentDb | ToggleFavoriteSource | ForgetSource | SourceTabRecents
            | SourceTabFavorites | SourceTabNext | SourceTabPrev => KeyGroup::Sources,
            ObjectSectionTables
            | ObjectSectionViews
            | ObjectSectionAdvanced
            | DetailTabPrev
            | DetailTabNext
            | DetailTabData
            | DetailTabSchema
            | DetailTabSql
            | DetailTabMeta => KeyGroup::Tabs,
            RunCountQuery | ClearQueryState | ReloadRuntimeConfig | OpenQueryInput
            | HScrollLeft | HScrollRight => KeyGroup::Data,
            Enter | ToggleActionsMenu | Yank | ExportCsv | StartFilter | ToggleHelp
            | ToggleInspectorJson => {
                KeyGroup::Actions
            }
        }
    }

    /// Descripción humana para la ayuda (es la misma que verás en la UI).
    pub const fn description(self) -> &'static str {
        use AppAction::{
            ClearQueryState, DetailTabData, DetailTabMeta, DetailTabNext, DetailTabPrev,
            DetailTabSchema, DetailTabSql, Enter, ExportCsv, FavoriteCurrentDb, FocusAdvanced,
            FocusDetail, FocusNext, FocusObjects, FocusPrev, FocusPreview, FocusSources,
            FocusTables, FocusViews, ForgetSource, HScrollLeft, HScrollRight, JumpToDetail,
            MoveDown, MoveUp, NextPage, ObjectSectionAdvanced, ObjectSectionTables,
            ObjectSectionViews, OpenQueryInput, PrevPage, QuitOrBack, Refresh, ReloadRuntimeConfig,
            RunCountQuery, SidebarFocusNext, SidebarFocusPrev, SourceTabFavorites, SourceTabNext,
            SourceTabPrev, SourceTabRecents, StartFilter, ToggleActionsMenu, ToggleCurrentPanel,
            ToggleFavoriteSource, ToggleHelp, ToggleInspectorJson, Yank,
        };
        match self {
            RunCountQuery => "Contar filas (query async)",
            ClearQueryState => "Limpiar resultado de query",
            ReloadRuntimeConfig => "Recargar config en caliente",
            OpenQueryInput => "Abrir input SQL (: historial)",
            QuitOrBack => "Volver / salir (por capas)",
            FocusPrev => "Panel anterior",
            FocusNext => "Panel siguiente",
            SidebarFocusPrev => "Sidebar anterior",
            SidebarFocusNext => "Sidebar siguiente",
            FocusSources => "Ir a Fuentes",
            FocusTables => "Ir a Tablas",
            FocusViews => "Ir a Vistas",
            FocusAdvanced => "Ir a Avanzado",
            FocusDetail => "Ir a Detalle",
            FocusObjects => "Ir a Tablas (obsoleto)",
            FocusPreview => "Ir a Detalle (obsoleto)",
            ToggleCurrentPanel => "Colapsar/expandir panel",
            JumpToDetail => "Saltar a Detalle",
            Refresh => "Refrescar",
            FavoriteCurrentDb => "Favoritear DB conectada",
            ToggleFavoriteSource => "Toggle favorito",
            ForgetSource => "Olvidar fuente",
            MoveUp => "Mover arriba",
            MoveDown => "Mover abajo",
            PrevPage => "Página anterior",
            NextPage => "Página siguiente",
            Enter => "Abrir / seleccionar",
            SourceTabRecents => "Tabs Fuentes: recientes",
            SourceTabFavorites => "Tabs Fuentes: favoritos",
            ObjectSectionTables => "Sección Tablas",
            ObjectSectionViews => "Sección Vistas",
            ObjectSectionAdvanced => "Sección Avanzado",
            DetailTabPrev => "Pestaña Detalle anterior",
            DetailTabNext => "Pestaña Detalle siguiente",
            DetailTabData => "Pestaña Datos",
            DetailTabSchema => "Pestaña Esquema",
            DetailTabSql => "Pestaña SQL",
            DetailTabMeta => "Pestaña Meta",
            SourceTabNext => "Pestaña Fuentes siguiente",
            SourceTabPrev => "Pestaña Fuentes anterior",
            ToggleActionsMenu => "Menú de acciones",
            Yank => "Copiar (yank)",
            ExportCsv => "Exportar CSV",
            StartFilter => "Filtrar /",
            ToggleInspectorJson => "Pares ↔ JSON (NoSQL)",
            HScrollLeft => "Scroll izq. (columnas)",
            HScrollRight => "Scroll der. (columnas)",
            ToggleHelp => "Ayuda de teclas",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: HashMap<String, AppAction>,
}

impl Keymap {
    pub fn load() -> Self {
        let mut keymap = Self::default();

        // Config GLOBAL + config POR PROYECTO (lazydb.toml hacia arriba
        // desde el CWD): el proyecto sobreescribe a la global. `set_binding`
        // reemplaza bindings previos de la misma acción, así la fusión es
        // natural: primero global, luego proyecto.
        let paths: [Option<std::path::PathBuf>; 2] =
            [Some(crate::config::config_file_path()), crate::config::find_project_config()];

        for path in paths.into_iter().flatten() {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };

            let Ok(parsed) = content.parse::<toml::Value>() else {
                continue;
            };

            let Some(table) = parsed.get("keys").and_then(toml::Value::as_table) else {
                continue;
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
        }

        keymap
    }

    fn set_binding(&mut self, token: &str, action: AppAction) {
        self.bindings.retain(|_, existing| *existing != action);
        self.bindings.insert(normalize_token(token), action);
    }

    /// Cantidad de bindings activos (para logs/depuración).
    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }

    /// Secciones de ayuda autogeneradas desde los bindings REALES:
    /// cada fila es `(teclas, descripción)` — si el usuario remapea una
    /// tecla en config.toml, la ayuda muestra lo que de verdad funciona.
    pub fn help_sections(&self) -> Vec<(&'static str, Vec<(String, &'static str)>)> {
        use std::collections::HashMap;

        let groups: [KeyGroup; 6] = [
            KeyGroup::Navigation,
            KeyGroup::Focus,
            KeyGroup::Sources,
            KeyGroup::Tabs,
            KeyGroup::Data,
            KeyGroup::Actions,
        ];

        groups
            .into_iter()
            .map(|group| {
                // acción → [tokens] (las variantes de una misma acción se
                // agrupan en una sola fila: "j, ↓" → Mover abajo)
                let mut by_action: HashMap<AppAction, Vec<String>> = HashMap::new();
                for (token, action) in &self.bindings {
                    if action.group() == group {
                        by_action.entry(*action).or_default().push(token.clone());
                    }
                }
                let mut rows: Vec<(String, &'static str)> = by_action
                    .into_iter()
                    .map(|(action, mut tokens)| {
                        tokens.sort();
                        (tokens.join(", "), action.description())
                    })
                    .collect();
                rows.sort_by(|a, b| a.1.cmp(b.1));
                (group.title(), rows)
            })
            .collect()
    }
}

impl Default for Keymap {
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // ── queries ──
        bindings.insert("ctrl+q".to_string(), AppAction::RunCountQuery);
        bindings.insert("ctrl+l".to_string(), AppAction::ClearQueryState);
        bindings.insert("ctrl+r".to_string(), AppAction::ReloadRuntimeConfig);
        bindings.insert(":".to_string(), AppAction::OpenQueryInput);

        // ── navegación global ──
        bindings.insert("esc".to_string(), AppAction::QuitOrBack);
        bindings.insert("q".to_string(), AppAction::QuitOrBack);
        bindings.insert("r".to_string(), AppAction::Refresh);
        bindings.insert("f".to_string(), AppAction::ToggleFavoriteSource);
        bindings.insert("d".to_string(), AppAction::ForgetSource);

        // ── foco entre paneles (Tab / Shift+Tab) ──
        bindings.insert("tab".to_string(), AppAction::FocusNext);
        bindings.insert("shift+tab".to_string(), AppAction::FocusPrev);

        // ── ir a panel específico (1-5) ──
        bindings.insert("1".to_string(), AppAction::FocusSources);
        bindings.insert("2".to_string(), AppAction::FocusTables);
        bindings.insert("3".to_string(), AppAction::FocusViews);
        bindings.insert("4".to_string(), AppAction::FocusAdvanced);
        bindings.insert("5".to_string(), AppAction::FocusDetail);

        // ── toggle panel (desactivado por defecto) ──

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
        bindings.insert("y".to_string(), AppAction::Yank);
        bindings.insert("e".to_string(), AppAction::ExportCsv);
        bindings.insert("/".to_string(), AppAction::StartFilter);
        bindings.insert("?".to_string(), AppAction::ToggleHelp);
        // Modal de detalles NoSQL: alternar pares ↔ JSON del documento
        bindings.insert("shift+j".to_string(), AppAction::ToggleInspectorJson);

        // ── scroll horizontal de columnas (Data tab) ──
        bindings.insert("shift+h".to_string(), AppAction::HScrollLeft);
        bindings.insert("shift+l".to_string(), AppAction::HScrollRight);

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
        // Mayúsculas → "shift+x" (independiente de si el terminal reporta
        // el modifier SHIFT explícito; Konsole a veces solo envía la letra)
        KeyCode::Char(ch) if ch.is_uppercase() => {
            Some(format!("shift+{}", ch.to_ascii_lowercase()))
        }
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
        "toggle_favorite_source" => Some(AppAction::ToggleFavoriteSource),
        "forget_source" => Some(AppAction::ForgetSource),
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
        "yank" => Some(AppAction::Yank),
        "export_csv" => Some(AppAction::ExportCsv),
        "start_filter" => Some(AppAction::StartFilter),
        "h_scroll_left" => Some(AppAction::HScrollLeft),
        "h_scroll_right" => Some(AppAction::HScrollRight),
        "toggle_help" => Some(AppAction::ToggleHelp),
        _ => None,
    }
}

fn normalize_token(token: &str) -> String {
    token.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_sections_agrupan_bindings_reales_por_grupo() {
        let k = Keymap::default();
        let sections = k.help_sections();

        // 6 grupos, todos presentes
        assert_eq!(sections.len(), 6);

        // "j, ↓" y "k, ↑" se agrupan en una sola fila por acción
        let nav = sections.iter().find(|(t, _)| *t == "Navegación").expect("grupo nav");
        assert!(nav.1.iter().any(|(keys, desc)| keys == "down, j" && *desc == "Mover abajo"));
        assert!(nav.1.iter().any(|(keys, desc)| keys == "k, up" && *desc == "Mover arriba"));

        // La ayuda `?` existe y está en Acciones
        let actions = sections.iter().find(|(t, _)| *t == "Acciones").expect("grupo acciones");
        assert!(actions.1.iter().any(|(keys, desc)| keys == "?" && *desc == "Ayuda de teclas"));

        // Toda fila tiene descripción no vacía y su grupo coincide con el título
        for (title, rows) in &sections {
            for (_, desc) in rows {
                assert!(!desc.is_empty(), "descripción vacía en {title}");
            }
        }
    }

    #[test]
    fn help_sections_reflejan_remapeos_del_usuario() {
        let mut k = Keymap::default();
        // El usuario remapea "Contar filas" a la tecla `p`
        k.set_binding("p", AppAction::RunCountQuery);

        let sections = k.help_sections();
        let data = sections.iter().find(|(t, _)| *t == "Datos y queries").expect("grupo datos");
        assert!(
            data.1.iter().any(|(keys, desc)| keys == "p" && *desc == "Contar filas (query async)")
        );

        // El binding viejo ctrl+q ya no aparece (se reemplazó)
        assert!(!data.1.iter().any(|(keys, _)| keys.contains("ctrl+q")));
    }

    #[test]
    fn cada_accion_del_keymap_default_tiene_descripcion() {
        let k = Keymap::default();
        for (_, rows) in k.help_sections() {
            for (_, desc) in rows {
                assert!(!desc.trim().is_empty());
            }
        }
    }
}
