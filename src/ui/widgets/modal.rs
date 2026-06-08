/// Widget Modal reutilizable: centrado en pantalla, fondo limpio, borde con título.
///
/// Uso:
/// - Menú de acciones (tecla `x`)
/// - Inspector de fila de datos (Enter/Click en tabla)
/// - Futuros diálogos (confirmaciones, inputs, etc.)
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

/// Renderiza un modal centrado.
///
/// `title`: título visible en el borde superior.
/// `lines`: contenido (una línea por ítem). Se trunca si no cabe.
/// `width_pct`: porcentaje del ancho de pantalla (ej. 60).
/// `height_pct`: porcentaje del alto de pantalla (ej. 50).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: &[String],
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
    let visible = usize::from(inner.height).min(lines.len());
    let content = lines.iter().take(visible).map(AsRef::as_ref).collect::<Vec<&str>>().join("\n");

    let paragraph = Paragraph::new(content).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);

    rect
}

const fn inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}
