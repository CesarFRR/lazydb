/// Layout engine: calcula constraints dinámicas para los paneles
/// según el tamaño de terminal y los modos de cada panel.
///
/// Principios:
/// - Eje horizontal: ≥80 cols → Detalle a la derecha; <80 → Detalle al fondo del stack.
/// - Eje vertical: solo el panel activo + Detalle se expanden; el resto colapsa a 3 líneas.
use ratatui::prelude::*;

use crate::app::PanelKind;

/// Umbral para que el Detalle migre al stack vertical
pub const NARROW_THRESHOLD: u16 = 80;

// ---------------------------------------------------------------------------
// Layout computado
// ---------------------------------------------------------------------------

/// Resultado del layout engine: rects para cada panel + footer.
#[derive(Clone, Debug)]
pub struct ComputedLayout {
    /// Rect del footer / status bar
    pub footer: Rect,
    /// Posiciones de cada panel en orden de renderizado
    pub panels: [(PanelKind, Rect); 5],
    /// ¿Modo angosto (detalle en stack vertical)?
    #[allow(dead_code)]
    pub is_narrow: bool,
}

impl Default for ComputedLayout {
    fn default() -> Self {
        Self {
            footer: Rect::default(),
            panels: [
                (PanelKind::Sources, Rect::default()),
                (PanelKind::Tables, Rect::default()),
                (PanelKind::Views, Rect::default()),
                (PanelKind::Advanced, Rect::default()),
                (PanelKind::Detail, Rect::default()),
            ],
            is_narrow: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Cálculo principal
// ---------------------------------------------------------------------------

/// Calcula el layout completo para la terminal actual.
///
/// `active_sidebar`: panel izquierdo que debe mantenerse expandido (siempre sidebar).
/// `current_focus`: panel que tiene el foco (puede ser Detail).
pub fn compute(
    width: u16,
    height: u16,
    active_sidebar: PanelKind,
    current_focus: PanelKind,
) -> ComputedLayout {
    debug_assert!(active_sidebar.is_sidebar());

    let is_narrow = width < NARROW_THRESHOLD;

    let footer_height: u16 = 1;

    let content_top = 0;
    let content_height = height.saturating_sub(footer_height);

    if content_height == 0 {
        return empty_layout(width, height, footer_height, is_narrow);
    }

    let panels = if is_narrow {
        narrow_layout(content_top, content_height, width, active_sidebar, current_focus)
    } else {
        wide_layout(content_top, content_height, width, active_sidebar)
    };

    ComputedLayout {
        footer: Rect::new(0, height.saturating_sub(footer_height), width, footer_height),
        panels,
        is_narrow,
    }
}

// ---------------------------------------------------------------------------
// Layout ancho (≥80 cols): sidebar izq + detalle der
// ---------------------------------------------------------------------------

fn wide_layout(
    top: u16,
    content_h: u16,
    width: u16,
    active_sidebar: PanelKind,
) -> [(PanelKind, Rect); 5] {
    let left_width = width.saturating_mul(33) / 100;
    let right_width = width.saturating_sub(left_width);

    let detail_rect = Rect::new(left_width, top, right_width, content_h);

    let sidebar_kinds: [PanelKind; 4] =
        [PanelKind::Sources, PanelKind::Tables, PanelKind::Views, PanelKind::Advanced];

    let left_rects = build_left_stack(top, content_h, left_width, &sidebar_kinds, active_sidebar);

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
    active_sidebar: PanelKind,
    _current_focus: PanelKind,
) -> [(PanelKind, Rect); 5] {
    let all_kinds: [PanelKind; 5] = [
        PanelKind::Sources,
        PanelKind::Tables,
        PanelKind::Views,
        PanelKind::Advanced,
        PanelKind::Detail,
    ];

    // Narrow mode: Detail takes remaining space, active sidebar gets 5 lines, rest 3 lines
    let rects = collapse_stack(top, content_h, width, &all_kinds, active_sidebar, true);

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
///   el stack) se expanden; los demás colapsan a 1 línea (Sources 5).
fn build_left_stack(
    top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
    active: PanelKind,
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

/// Todos los paneles reciben altura proporcional.
/// En modo angosto (5 paneles), Detalle ocupa 2 partes y los demás 1 parte.
fn equitative_stack(
    mut top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
) -> Vec<(PanelKind, Rect)> {
    // Peso ×2 para Detalle en modo angosto
    let total_parts: u16 =
        kinds.iter().map(|k| if *k == PanelKind::Detail { 2u16 } else { 1u16 }).sum();

    let unit = total_h.checked_div(total_parts).unwrap_or(total_h);

    let mut rects = Vec::with_capacity(kinds.len());

    for (i, &kind) in kinds.iter().enumerate() {
        let weight = if kind == PanelKind::Detail { 2u16 } else { 1u16 };
        let h = if i == kinds.len() - 1 { total_h.saturating_sub(top) } else { unit * weight };
        rects.push((kind, Rect::new(0, top, width, h)));
        top += h;
    }

    rects
}

/// Solo el panel activo + Detalle se expanden; colapsados a 1 línea (Sources 5).
fn collapse_stack(
    mut top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
    active: PanelKind,
    has_detail: bool,
) -> Vec<(PanelKind, Rect)> {
    // Calcular altura fija de los paneles colapsados (Sources=5, otros=1)
    let mut fixed_lines: u16 = 0;
    for &k in kinds {
        if k != active && k != PanelKind::Detail {
            fixed_lines += if k == PanelKind::Sources { 3 } else { 1 };
        }
    }

    let (sidebar_h, remaining) = if has_detail {
        let sh = (total_h / 4).max(5); // sidebar recordado = 1/4 del alto
        (sh, total_h.saturating_sub(fixed_lines).saturating_sub(sh))
    } else {
        let rem = total_h.saturating_sub(fixed_lines);
        (rem, rem) // wide: activo toma el resto
    };

    let mut rects = Vec::with_capacity(kinds.len());

    for &kind in kinds {
        let h = if kind == PanelKind::Detail {
            remaining.max(5).min(total_h.saturating_sub(top))
        } else if kind == active {
            sidebar_h.min(total_h.saturating_sub(top))
        } else if kind == PanelKind::Sources {
            3 // Sources mínimo: borde + 1 ítem
        } else {
            1 // colapsado: 1 línea
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

fn empty_layout(width: u16, height: u16, footer_h: u16, is_narrow: bool) -> ComputedLayout {
    ComputedLayout {
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
    }
}
