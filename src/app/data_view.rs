//! Estado del Data view (Fase 3 del refactor del monolito): el preview
//! (filas tipadas + fallback de strings), el tab activo y la paginación.
//!
//! A diferencia de Sources/Connection, este estado es el NÚCLEO COMPARTIDO
//! (lo leen la UI, el query runner y los handlers de mouse): por eso se
//! extrae como struct de campos `pub` y la lógica (`spawn_preview`/`poll`/
//! scrolls) queda en el controller operando sobre `self.data_view` —
//! evitar el borrow hell del render pesa más que mover los métodos.

use crate::app::controller::DetailTab;

/// Estado del panel central (Detail): preview, tabs y paginación.
pub struct DataViewState {
    /// Líneas del preview (fallback de strings: List de 1 columna, mensajes).
    pub preview_rows: Vec<String>,
    /// Celdas TIPADAS del Data tab: fuente de verdad del render 2D.
    /// `None` cuando la vista no es una tabla (mensajes, SQL, schema).
    pub preview_data: Option<crate::db::TableData>,
    /// Tab activo del panel Detail (Data/Schema/Sql/Meta).
    pub detail_tab: DetailTab,
    /// Total de filas del objeto (COUNT real, para el label X/Y).
    pub total_rows: u32,
    /// Filas por página (ajustado dinámicamente al espacio disponible).
    pub rows_per_page: u32,
    /// Página actual del dataset (paginación del preview).
    pub current_page: u32,
    /// Offset global de la primera fila cargada (scroll infinito).
    pub preview_loaded_offset: u32,
    /// `true` cuando el preview muestra el resultado de una query libre.
    pub query_mode: bool,
    /// Columna de orden activa (None = orden natural del motor).
    pub sort_column: Option<String>,
    /// `true` = ascendente (▴), `false` = descendente (▾).
    pub sort_asc: bool,
}

impl DataViewState {
    pub const fn new() -> Self {
        Self {
            preview_rows: Vec::new(),
            preview_data: None,
            detail_tab: DetailTab::Data,
            total_rows: 0,
            rows_per_page: 50,
            current_page: 0,
            preview_loaded_offset: 0,
            query_mode: false,
            sort_column: None,
            sort_asc: true,
        }
    }
}
