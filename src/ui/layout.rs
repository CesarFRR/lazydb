/// Layout engine: calcula constraints dinámicas para los paneles
/// según el tamaño de terminal y los modos de cada panel.
///
/// Principios:
/// - Eje horizontal: ≥80 cols → Detalle a la derecha; <80 → Detalle al fondo del stack.
/// - Eje vertical: solo el panel activo + Detalle se expanden; el resto colapsa a 3 líneas.
use ratatui::prelude::*;

use crate::app::{PanelKind, PanelMode};

/// Umbral para que el Detalle migre al stack vertical
pub const NARROW_THRESHOLD: u16 = 80;

/// Alto mínimo de terminal para header de 2 líneas
pub const COMPACT_HEIGHT: u16 = 14;

// ---------------------------------------------------------------------------
// Layout computado
// ---------------------------------------------------------------------------

/// Resultado del layout engine: rects para cada panel + cabecera + footer.
#[derive(Clone, Debug)]
pub struct ComputedLayout {
    /// Rect de la cabecera
    pub header: Rect,
    /// Rect del footer / status bar
    pub footer: Rect,
    /// Posiciones de cada panel en orden de renderizado
    pub panels: [(PanelKind, Rect); 5],
    /// ¿Modo angosto (detalle en stack vertical)?
    #[allow(dead_code)]
    pub is_narrow: bool,
    /// ¿Altura compacta (header de 1 línea)?
    pub compact_height: bool,
}

impl Default for ComputedLayout {
    fn default() -> Self {
        Self {
            header: Rect::default(),
            footer: Rect::default(),
            panels: [
                (PanelKind::Sources, Rect::default()),
                (PanelKind::Tables, Rect::default()),
                (PanelKind::Views, Rect::default()),
                (PanelKind::Advanced, Rect::default()),
                (PanelKind::Detail, Rect::default()),
            ],
            is_narrow: false,
            compact_height: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Cálculo principal
// ---------------------------------------------------------------------------

/// Calcula el layout completo para la terminal actual.
///
/// `active`: el panel que tiene el foco (se expande).
/// `panel_modes`: modos de cada panel (Sources..Detail).
pub fn compute(
    width: u16,
    height: u16,
    active: PanelKind,
    panel_modes: &[(PanelKind, PanelMode); 5],
) -> ComputedLayout {
    let compact_height = height < COMPACT_HEIGHT;
    let is_narrow = width < NARROW_THRESHOLD;

    let header_height: u16 = if compact_height { 1 } else { 2 };
    let footer_height: u16 = 1;

    let content_top = header_height;
    let content_height = height.saturating_sub(header_height + footer_height);

    if content_height == 0 {
        return empty_layout(
            width,
            height,
            header_height,
            footer_height,
            is_narrow,
            compact_height,
        );
    }

    let panels = if is_narrow {
        narrow_layout(content_top, content_height, width, active, panel_modes)
    } else {
        wide_layout(content_top, content_height, width, active, panel_modes)
    };

    ComputedLayout {
        header: Rect::new(0, 0, width, header_height),
        footer: Rect::new(0, height.saturating_sub(footer_height), width, footer_height),
        panels,
        is_narrow,
        compact_height,
    }
}

// ---------------------------------------------------------------------------
// Layout ancho (≥80 cols): sidebar izq + detalle der
// ---------------------------------------------------------------------------

fn wide_layout(
    top: u16,
    content_h: u16,
    width: u16,
    active: PanelKind,
    panel_modes: &[(PanelKind, PanelMode); 5],
) -> [(PanelKind, Rect); 5] {
    let left_width = width.saturating_mul(33) / 100;
    let right_width = width.saturating_sub(left_width);

    // Panel de Detalle siempre a la derecha, altura completa
    let detail_rect = Rect::new(left_width, top, right_width, content_h);

    // 4 paneles izquierdos en columna
    let sidebar_kinds: [PanelKind; 4] =
        [PanelKind::Sources, PanelKind::Tables, PanelKind::Views, PanelKind::Advanced];

    let left_rects =
        build_left_stack(top, content_h, left_width, &sidebar_kinds, active, panel_modes);

    [
        find_panel_rect(&left_rects, PanelKind::Sources),
        find_panel_rect(&left_rects, PanelKind::Tables),
        find_panel_rect(&left_rects, PanelKind::Views),
        find_panel_rect(&left_rects, PanelKind::Advanced),
        (PanelKind::Detail, detail_rect),
    ]
}

// ---------------------------------------------------------------------------
// Layout angosto (<80 cols): todo en una columna, Detalle al fondo
// ---------------------------------------------------------------------------

fn narrow_layout(
    top: u16,
    content_h: u16,
    width: u16,
    active: PanelKind,
    panel_modes: &[(PanelKind, PanelMode); 5],
) -> [(PanelKind, Rect); 5] {
    let all_kinds: [PanelKind; 5] = [
        PanelKind::Sources,
        PanelKind::Tables,
        PanelKind::Views,
        PanelKind::Advanced,
        PanelKind::Detail,
    ];

    let rects = build_left_stack(top, content_h, width, &all_kinds, active, panel_modes);

    [
        find_panel_rect(&rects, PanelKind::Sources),
        find_panel_rect(&rects, PanelKind::Tables),
        find_panel_rect(&rects, PanelKind::Views),
        find_panel_rect(&rects, PanelKind::Advanced),
        find_panel_rect(&rects, PanelKind::Detail),
    ]
}

// ---------------------------------------------------------------------------
// Stack vertical genérico
// ---------------------------------------------------------------------------

/// Construye un stack vertical de paneles.
///
/// Dos modos:
/// - **Equitativo**: cuando hay altura suficiente, todos los paneles se reparten
///   el espacio por igual (`total_h` / n). El último toma el resto.
/// - **Colapso**: cuando falta altura, solo el panel activo + Detalle (si está en
///   el stack) se expanden; los demás colapsan a 3 líneas.
fn build_left_stack(
    top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
    active: PanelKind,
    _panel_modes: &[(PanelKind, PanelMode); 5],
) -> Vec<(PanelKind, Rect)> {
    #[allow(clippy::cast_possible_truncation)]
    let n = kinds.len() as u16;
    let has_detail = kinds.contains(&PanelKind::Detail);

    // Altura mínima para modo equitativo:
    // — los paneles "normales" necesitan al menos 5 líneas (Expanded)
    // — el detalle necesita al menos 5 líneas
    // — los demás colapsados necesitarían 3, pero en modo equitativo todos se expanden
    let min_equitative = n * 5;

    if total_h >= min_equitative {
        equitative_stack(top, total_h, width, kinds)
    } else {
        collapse_stack(top, total_h, width, kinds, active, has_detail)
    }
}

/// Todos los paneles reciben la misma altura. El último toma el sobrante.
fn equitative_stack(
    mut top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
) -> Vec<(PanelKind, Rect)> {
    #[allow(clippy::cast_possible_truncation)]
    let n = kinds.len() as u16;
    let each = total_h / n;
    let mut rects = Vec::with_capacity(kinds.len());

    for (i, &kind) in kinds.iter().enumerate() {
        let h = if i == kinds.len() - 1 { total_h.saturating_sub(top) } else { each };
        rects.push((kind, Rect::new(0, top, width, h)));
        top += h;
    }

    rects
}

/// Solo el panel activo + Detalle se expanden; el resto colapsa a 3 líneas.
fn collapse_stack(
    mut top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
    active: PanelKind,
    has_detail: bool,
) -> Vec<(PanelKind, Rect)> {
    let expandable_count: u16 = if has_detail && active != PanelKind::Detail { 2 } else { 1 };
    #[allow(clippy::cast_possible_truncation)]
    let collapsed_count: u16 =
        kinds.iter().filter(|k| **k != active && **k != PanelKind::Detail).count() as u16;
    let collapsed_lines = collapsed_count * 3;
    let remaining = total_h.saturating_sub(collapsed_lines);
    let per_expanded = if expandable_count > 0 && remaining > 0 {
        remaining / expandable_count
    } else {
        remaining
    };

    let mut rects = Vec::with_capacity(kinds.len());

    for (i, &kind) in kinds.iter().enumerate() {
        let is_last = i == kinds.len() - 1;

        let h = if kind == PanelKind::Detail || kind == active {
            let base = if kind == PanelKind::Detail && is_last {
                total_h.saturating_sub(top)
            } else {
                per_expanded
            };
            base.max(5) // mínimo 5 líneas para expanded
        } else {
            3 // colapsado
        };

        let h = h.min(total_h.saturating_sub(top));
        rects.push((kind, Rect::new(0, top, width, h)));
        top += h;

        if top >= total_h {
            break;
        }
    }

    rects
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_panel_rect(rects: &[(PanelKind, Rect)], kind: PanelKind) -> (PanelKind, Rect) {
    rects.iter().find(|(k, _)| *k == kind).copied().unwrap_or_else(|| (kind, Rect::default()))
}

fn empty_layout(
    width: u16,
    height: u16,
    header_h: u16,
    footer_h: u16,
    is_narrow: bool,
    compact_height: bool,
) -> ComputedLayout {
    ComputedLayout {
        header: Rect::new(0, 0, width, header_h),
        footer: Rect::new(0, height.saturating_sub(footer_h), width, footer_h),
        panels: [(PanelKind::Sources, Rect::default()); 5]
            .into_iter()
            .enumerate()
            .map(|(i, _)| {
                let kind = PanelKind::ALL[i];
                (kind, Rect::default())
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or_else(|_: Vec<_>| [(PanelKind::Sources, Rect::default()); 5]),
        is_narrow,
        compact_height,
    }
}
