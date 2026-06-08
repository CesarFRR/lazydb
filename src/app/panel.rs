use std::cell::Cell;
///
/// Cada "cuadrito" de la UI (Fuentes, Tablas, Vistas, Avanzado, Detalle)
/// tiene un modo de renderizado y es tratado como un panel independiente
/// con foco y scroll propios.

// ---------------------------------------------------------------------------
// PanelKind
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelKind {
    /// Panel de fuentes/conexiones (Sources)
    Sources,
    /// Panel de tablas
    Tables,
    /// Panel de vistas
    Views,
    /// Panel de objetos avanzados (índices, triggers)
    Advanced,
    /// Panel de detalle (datos, esquema, SQL, meta)
    Detail,
}

impl PanelKind {
    /// Título corto en inglés (usado en cabecera)
    #[allow(dead_code)]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Sources => "Sources",
            Self::Tables => "Tables",
            Self::Views => "Views",
            Self::Advanced => "Advanced",
            Self::Detail => "Detail",
        }
    }

    /// Etiqueta amigable en español
    #[allow(dead_code)]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sources => "Fuentes",
            Self::Tables => "Tablas",
            Self::Views => "Vistas",
            Self::Advanced => "Avanzado",
            Self::Detail => "Detalle",
        }
    }

    /// Todos los paneles en orden de navegación con Tab
    pub const ALL: [Self; 5] =
        [Self::Sources, Self::Tables, Self::Views, Self::Advanced, Self::Detail];

    /// Siguiente panel en el ciclo de foco
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Sources => Self::Tables,
            Self::Tables => Self::Views,
            Self::Views => Self::Advanced,
            Self::Advanced => Self::Detail,
            Self::Detail => Self::Sources,
        }
    }

    /// Anterior panel en el ciclo de foco
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Sources => Self::Detail,
            Self::Tables => Self::Sources,
            Self::Views => Self::Tables,
            Self::Advanced => Self::Views,
            Self::Detail => Self::Advanced,
        }
    }

    /// ¿Es un panel de la barra lateral izquierda?
    pub const fn is_sidebar(self) -> bool {
        !matches!(self, Self::Detail)
    }

    /// Número de atajo (1-5) para teclas de acceso directo
    pub const fn number(self) -> u8 {
        match self {
            Self::Sources => 1,
            Self::Tables => 2,
            Self::Views => 3,
            Self::Advanced => 4,
            Self::Detail => 5,
        }
    }
}

// ---------------------------------------------------------------------------
// PanelMode
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelMode {
    /// Solo borde + título (3 líneas de alto)
    Collapsed,
    /// Título + 1 ítem seleccionado (~5 líneas). Estado interno, no expuesto al usuario.
    #[allow(dead_code)]
    Minimal,
    /// Muestra el contenido completo, creciendo hasta un tope máximo
    #[default]
    Expanded,
    /// Altura fija explícita en líneas (incluye bordes)
    #[allow(dead_code)]
    Fixed(u16),
}

impl PanelMode {
    /// Altura mínima que ocupa este panel (bordes incluidos)
    #[allow(dead_code)]
    pub const fn min_lines(self) -> u16 {
        match self {
            Self::Collapsed => 3,
            Self::Minimal | Self::Expanded => 5,
            Self::Fixed(h) => h,
        }
    }

    /// Alterna entre Collapsed y Expanded
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Collapsed => Self::Expanded,
            Self::Expanded => Self::Collapsed,
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

/// Estado de un panel individual.
/// Los datos (items) se almacenan en `App`; aquí solo va el estado visual.
#[derive(Clone, Debug)]
pub struct Panel {
    /// Identificador del panel
    pub kind: PanelKind,
    /// Modo actual de renderizado
    pub mode: PanelMode,
    /// Índice del ítem seleccionado (cursor)
    pub selected_idx: usize,
    /// Offset de scroll (primera fila visible)
    pub scroll_offset: Cell<usize>,
}

impl Panel {
    pub fn new(kind: PanelKind) -> Self {
        Self { kind, mode: PanelMode::default(), selected_idx: 0, scroll_offset: Cell::new(0) }
    }

    /// Paneles izquierdos arrancan expandidos, Detalle también.
    /// En la práctica solo 1 izquierdo estará expandido a la vez (el activo).
    pub fn new_sidebar(kind: PanelKind) -> Self {
        debug_assert!(kind.is_sidebar());
        Self { kind, mode: PanelMode::Expanded, selected_idx: 0, scroll_offset: Cell::new(0) }
    }
}
