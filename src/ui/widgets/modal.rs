/// Widget Modal reutilizable: centrado en pantalla, fondo limpio, borde con título,
/// contenido desplazable con scroll.
///
/// Uso:
/// - Menú de acciones (tecla `x`)
/// - Inspector de fila de datos (Enter/Click en tabla)
/// - Futuros diálogos (confirmaciones, inputs, etc.)
use ratatui::{
    prelude::*,
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
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

    // Scrollbar
    if lines.len() > visible {
        let scrollbar_state = ScrollbarState::new(lines.len())
            .viewport_content_length(visible)
            .position(scroll.offset);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(ratatui::symbols::scrollbar::VERTICAL);
        let mut state = scrollbar_state;
        frame.render_stateful_widget(scrollbar, rect, &mut state);
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
