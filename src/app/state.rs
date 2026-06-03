use crate::app::panel::{PanelKind, PanelMode};

/// Estado específico de la UI que persiste entre sesiones o se comparte entre componentes.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct UiState {
    /// Modo por defecto para paneles colapsables
    pub default_panel_mode: PanelMode,
    /// Último panel activo (para restaurar al abrir)
    pub last_active_panel: Option<PanelKind>,
}

impl UiState {
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self { default_panel_mode: PanelMode::Expanded, last_active_panel: None }
    }
}
