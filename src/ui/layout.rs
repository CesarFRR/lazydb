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

fn build_left_stack(
    mut top: u16,
    total_h: u16,
    width: u16,
    kinds: &[PanelKind],
    active: PanelKind,
    panel_modes: &[(PanelKind, PanelMode); 5],
) -> Vec<(PanelKind, Rect)> {
    // 1. Calcular altura que necesitan los paneles colapsados
    let collapsed_count: usize =
        kinds.iter().filter(|k| **k != active && **k != PanelKind::Detail).count();
    #[allow(clippy::cast_possible_truncation)]
    let collapsed_lines: u16 = collapsed_count as u16 * 3;

    // 2. El espacio restante se reparte entre: activo + Detalle (si Detail está en el stack)
    let has_detail_in_stack = kinds.contains(&PanelKind::Detail);
    let expandable_count: u16 =
        if has_detail_in_stack && active != PanelKind::Detail { 2 } else { 1 };

    let remaining = total_h.saturating_sub(collapsed_lines);
    let per_expanded = if expandable_count > 0 && remaining > 0 {
        remaining / expandable_count
    } else {
        remaining
    };

    let mut rects = Vec::with_capacity(kinds.len());

    for (i, &kind) in kinds.iter().enumerate() {
        let is_last = i == kinds.len() - 1;
        let panel_mode = lookup_mode(panel_modes, kind);

        let panel_h = if kind == PanelKind::Detail {
            // Detalle: siempre el máximo posible
            if is_last {
                total_h.saturating_sub(
                    top.saturating_sub(
                        kinds.first().map_or(0, |_| {
                            rects.first().map_or(top, |r: &(PanelKind, Rect)| r.1.y)
                        }),
                    ),
                )
            } else {
                per_expanded
            }
        } else if kind == active {
            per_expanded
        } else {
            match panel_mode {
                PanelMode::Collapsed => 3,
                PanelMode::Minimal => 5,
                PanelMode::Expanded => total_h.saturating_sub(collapsed_lines).min(per_expanded),
                PanelMode::Fixed(h) => h,
            }
        };

        let panel_h = panel_h.max(panel_mode.min_lines()).min(total_h.saturating_sub(top));
        let h = if is_last {
            // Último panel toma todo el espacio restante
            (top + total_h).saturating_sub(top)
        } else {
            panel_h
        };

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

fn lookup_mode(modes: &[(PanelKind, PanelMode); 5], kind: PanelKind) -> PanelMode {
    modes.iter().find(|(k, _)| *k == kind).map_or(PanelMode::Expanded, |(_, m)| *m)
}

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
