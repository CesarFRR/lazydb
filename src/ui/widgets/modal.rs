/// Widget Modal reutilizable: centrado en pantalla, fondo limpio, borde con título,
/// contenido desplazable con scroll.
///
/// Uso:
/// - Menú de acciones (tecla `x`)
/// - Inspector de fila de datos (Enter/Click en tabla)
/// - Futuros diálogos (confirmaciones, inputs, etc.)
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState, Wrap},
};

use crate::ui::theme::THEME;
/// Scroll state externo: el caller lo mantiene y lo pasa cada frame.
#[derive(Clone, Debug, Default)]
pub struct ModalScroll {
    pub offset: usize,
}

impl ModalScroll {
    #[allow(clippy::missing_const_for_fn)]
    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1);
    }

    /// Desplaza `n` líneas hacia arriba (help modal).
    #[allow(clippy::missing_const_for_fn)]
    pub fn up(&mut self, n: usize) {
        self.offset = self.offset.saturating_sub(n);
    }

    /// Desplaza `n` líneas hacia abajo (help modal).
    #[allow(clippy::missing_const_for_fn)]
    pub fn down(&mut self, n: usize) {
        self.offset = self.offset.saturating_add(n);
    }

    /// Clampea el offset al máximo real del contenido. Lo llama el render en
    /// cada frame (anti "scroll fantasma": si el offset creció más allá del
    /// final, el scroll up consumiría ese excedente antes de moverse).
    #[must_use]
    pub fn clamp_to(&mut self, max: usize) -> usize {
        self.offset = self.offset.min(max);
        self.offset
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

/// Geometría centrada del modal. La comparte el render (para dibujar) y el
/// controller (para el hit-testing del scrollbar interior y del drag).
#[must_use]
pub const fn geometry(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
    let width = area.width.saturating_mul(width_pct) / 100;
    let height = area.height.saturating_mul(height_pct) / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

/// Ancho de clave y valor dentro del inner del modal (misma fórmula en el
/// render y en el controller para que el hit-testing del drag coincida).
#[must_use]
pub fn table_geometry(inner: Rect) -> (u16, u16) {
    let key_w = (inner.width.saturating_sub(3) * 40 / 100).max(8);
    let val_w = inner.width.saturating_sub(key_w).saturating_sub(3);
    (key_w, val_w)
}

/// Convierte pares (clave, valor) en filas ya expandidas: un valor
/// multilínea (expandido por el inspector) se parte en varias filas. Misma
/// lógica en el render y en el controller (para medir `content_len`).
pub fn expand_pairs(
    pairs: &[(String, String)],
    key_w: usize,
    val_w: usize,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in pairs {
        let key = crate::ui::widgets::panel::truncate_middle(k, key_w);
        let mut val_lines: Vec<String> = Vec::new();
        for part in v.split('\n') {
            val_lines.extend(crate::ui::widgets::panel::wrap_text(part, val_w));
        }
        if val_lines.is_empty() {
            val_lines.push(String::new());
        }
        for (i, line) in val_lines.into_iter().enumerate() {
            let col0 = if i == 0 { key.clone() } else { String::new() };
            out.push((col0, line));
        }
    }
    out
}

/// Renderiza un modal centrado con líneas ya estilizadas (`Line`/`Span`).
///
/// `border_style: None` usa el borde neutro del tema; `Some(style)` permite
/// colores semánticos (rojo para popups de error, etc.).
///
/// Devuelve el inner rect (área de contenido sin bordes) para cálculos futuros.
///
/// El scrollbar se dibuja DENTRO del modal (última columna del inner), no en
/// el borde: así el hit-testing del drag es inequívoco (columna interior del
/// modal → scroll del modal; fuera → paneles).
///
/// Recibe `&mut ModalScroll`: el render CLAMPEA el offset del estado al máximo
/// real del contenido. Sin esto, el offset puede crecer más allá del final
/// (rueda/teclas) y al hacer scroll hacia arriba el contenido "tarda" en
/// moverse (scroll fantasma: consume el excedente antes de bajar).
#[allow(clippy::too_many_arguments)]
pub fn render_lines(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: &[Line<'_>],
    scroll: &mut ModalScroll,
    width_pct: u16,
    height_pct: u16,
    border_style: Option<Style>,
) -> Rect {
    let rect = geometry(area, width_pct, height_pct);

    frame.render_widget(Clear, rect);

    let border_style = border_style.unwrap_or_else(|| Style::default().fg(THEME.border));
    let block =
        Block::default().title(title.to_string()).borders(Borders::ALL).border_style(border_style);

    let inner = inner_area(rect);
    let visible = usize::from(inner.height.max(1));

    // Clamp: nunca dejar el offset más allá del contenido. Se corrige TAMBIÉN
    // el estado (no solo el dibujo) para que el scroll up reaccione al
    // instante (sin scroll fantasma acumulado).
    let offset = scroll.clamp_to(lines.len().saturating_sub(visible));

    // Truncar líneas al offset
    let visible_lines: Vec<Line<'_>> = lines.iter().skip(offset).take(visible).cloned().collect();

    // Contenido 1 columna más angosto para dejar sitio al scrollbar interior
    let content_rect = Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), rect.height);

    let paragraph = Paragraph::new(visible_lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, content_rect);

    // Scrollbar manual (misma lógica/estilo que los paneles: thumb de largo
    // fijo que recorre el 100% del track). area reducida → queda en la última
    // columna del inner (dentro del modal).
    if lines.len() > visible {
        crate::ui::widgets::panel::draw_v_scrollbar(frame, content_rect, lines.len(), offset);
    }

    inner
}

#[must_use]
#[allow(clippy::redundant_pub_crate)]
pub(crate) const fn inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

// ---------------------------------------------------------------------------
// Modal con Table (2 columnas: clave | valor)
// ---------------------------------------------------------------------------

/// Renderiza un modal con tabla 2 columnas. Valores largos se parten en múltiples filas.
///
/// El scrollbar se dibuja DENTRO del modal (última columna del inner) y el
/// `TableState` usa `with_offset` con el MISMO offset del scrollbar manual:
/// así la tabla y la barra nunca divergen.
///
/// Recibe `&mut ModalScroll` por el mismo motivo que `render_lines`: el
/// render clampea el offset del estado al máximo real (anti scroll fantasma).
pub fn render_table(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    pairs: &[(String, String)],
    scroll: &mut ModalScroll,
    width_pct: u16,
    height_pct: u16,
) -> Rect {
    let rect = geometry(area, width_pct, height_pct);

    frame.render_widget(Clear, rect);

    let border_style = Style::default().fg(THEME.border);
    let block =
        Block::default().title(title.to_string()).borders(Borders::ALL).border_style(border_style);

    let inner = inner_area(rect);
    let (key_w, val_w) = table_geometry(inner);

    let header =
        Row::new(["Columna", "Valor"]).style(Style::default().fg(THEME.selection)).height(1);

    // Expandir pares en filas: valor largo → múltiples filas. Primero se
    // respetan los saltos de línea explícitos (valores expandidos del
    // inspector: structs/maps/lists multilínea) y luego se hace wrap.
    let expanded = expand_pairs(pairs, key_w as usize, val_w as usize);
    let rows: Vec<Row<'_>> =
        expanded.iter().map(|(k, v)| Row::new(vec![k.clone(), v.clone()])).collect();

    let content_len = rows.len().saturating_add(1); // +1 header
    let visible = usize::from(inner.height.max(1));
    // Clamp: mismo offset para tabla y scrollbar → sincronía exacta. También
    // corrige el estado (anti scroll fantasma, ver `render_lines`).
    let offset = scroll.clamp_to(content_len.saturating_sub(visible));

    let widths = [Constraint::Length(key_w), Constraint::Length(val_w)];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    let mut state = TableState::default().with_offset(offset).with_selected(Some(offset));
    // Contenido 1 columna más angosto para dejar sitio al scrollbar interior
    let content_rect = Rect::new(rect.x, rect.y, rect.width.saturating_sub(1), rect.height);
    frame.render_stateful_widget(table, content_rect, &mut state);

    // Scrollbar manual dentro del modal (área reducida → última columna del inner)
    if content_len > visible {
        crate::ui::widgets::panel::draw_v_scrollbar(frame, content_rect, content_len, offset);
    }

    inner
}

#[cfg(test)]
mod tests {
    use super::ModalScroll;

    /// El scroll fantasma: offset que creció más allá del contenido debe
    /// volver al máximo real con el clamp (para que scroll up reaccione
    /// al instante, sin consumir el excedente).
    #[test]
    fn clamp_elimina_scroll_fantasma() {
        let mut s = ModalScroll::default();
        // Contenido: 10 líneas, viewport 5 → max offset 5.
        for _ in 0..100 {
            s.scroll_down();
        }
        assert_eq!(s.clamp_to(5), 5, "el excedente acumulado se corrige");
        assert_eq!(s.offset, 5);

        // Ya clampeado: scroll down no puede volver a inflar el offset
        // (el render lo corregirá igual, pero el estado queda sano).
        s.scroll_down();
        assert_eq!(s.clamp_to(5), 5);
        assert_eq!(s.offset, 5);

        // Scroll up desde el máximo reacciona de inmediato
        s.scroll_up();
        assert_eq!(s.clamp_to(5), 4);
        assert_eq!(s.offset, 4);
    }
}
