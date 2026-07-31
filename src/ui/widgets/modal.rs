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

    #[allow(clippy::missing_const_for_fn)]
    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

/// Renderiza un modal centrado con scroll vertical.
///
/// Devuelve el inner rect (área de contenido sin bordes) para cálculos futuros.
#[allow(dead_code)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: &[String],
    scroll: &ModalScroll,
    width_pct: u16,
    height_pct: u16,
) -> Rect {
    let width = area.width.saturating_mul(width_pct) / 100;
    let height = area.height.saturating_mul(height_pct) / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    frame.render_widget(Clear, rect);

    let border_style = Style::default().fg(Color::Cyan);
    let block =
        Block::default().title(title.to_string()).borders(Borders::ALL).border_style(border_style);

    let inner = inner_area(rect);
    let visible = usize::from(inner.height.max(1));

    // Truncar líneas al offset
    let visible_lines: Vec<&str> =
        lines.iter().skip(scroll.offset).take(visible).map(AsRef::as_ref).collect();

    let content = visible_lines.join("\n");

    let paragraph = Paragraph::new(content).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);

    // Scrollbar manual (misma lógica/estilo que los paneles: thumb de largo
    // fijo que recorre el 100% del track)
    if lines.len() > visible {
        crate::ui::widgets::panel::draw_v_scrollbar(frame, rect, lines.len(), scroll.offset);
    }

    inner
}

#[must_use]
const fn inner_area(area: Rect) -> Rect {
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
pub fn render_table(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    pairs: &[(String, String)],
    scroll: &ModalScroll,
    width_pct: u16,
    height_pct: u16,
) -> Rect {
    let width = area.width.saturating_mul(width_pct) / 100;
    let height = area.height.saturating_mul(height_pct) / 100;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    frame.render_widget(Clear, rect);

    let border_style = Style::default().fg(Color::Cyan);
    let block =
        Block::default().title(title.to_string()).borders(Borders::ALL).border_style(border_style);

    let inner = inner_area(rect);
    let key_w = (inner.width.saturating_sub(3) * 40 / 100).max(8);
    let val_w = inner.width.saturating_sub(key_w).saturating_sub(3);

    let header = Row::new(["Columna", "Valor"]).style(Style::default().fg(Color::Cyan)).height(1);

    // Expandir pares en filas: valor largo → múltiples filas
    let mut rows: Vec<Row<'_>> = Vec::new();
    for (k, v) in pairs {
        let key = crate::ui::widgets::panel::truncate_middle(k, key_w as usize);
        let val_lines = crate::ui::widgets::panel::wrap_text(v, val_w as usize);
        for (i, line) in val_lines.into_iter().enumerate() {
            let col0 = if i == 0 { key.clone() } else { String::new() };
            rows.push(Row::new(vec![col0, line]));
        }
    }

    let content_len = rows.len().saturating_add(1); // +1 header
    let widths = [Constraint::Length(key_w), Constraint::Length(val_w)];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    let mut state = TableState::default().with_selected(Some(scroll.offset));
    frame.render_stateful_widget(table, rect, &mut state);

    // Scrollbar manual (misma lógica/estilo que los paneles: thumb de largo
    // fijo que recorre el 100% del track)
    let visible = usize::from(inner.height.max(1));
    if content_len > visible {
        crate::ui::widgets::panel::draw_v_scrollbar(frame, rect, content_len, scroll.offset);
    }

    inner
}
