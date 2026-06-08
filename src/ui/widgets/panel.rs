/// Widget para renderizar un `Panel` de la UI.
///
/// Soporta los modos Collapsed (solo borde + título) y Expanded (contenido completo
/// con scroll). El modo Minimal está implementado pero no expuesto al usuario.
use ratatui::{
    prelude::*,
    text::Line as RatLine,
    text::Span,
    widgets::{
        Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};

use crate::app::{PanelKind, PanelMode};

/// Trunca un string con puntos suspensivos en el medio si excede `max_w`.
/// Ej: "Luis Hernando Garcia..." → "Luis Hernan.....o Garcia"
#[allow(dead_code)]
pub fn truncate_middle(text: &str, max_w: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_w {
        return text.to_string();
    }
    if max_w < 5 {
        return chars.iter().take(max_w).collect::<String>();
    }
    let half = (max_w - 3) / 2;
    let left: String = chars.iter().take(half).collect();
    let right: String = chars.iter().rev().take(half).collect::<String>().chars().rev().collect(); // revert the reversed collect
    format!("{left}...{right}")
}

/// Parte un texto en líneas de hasta `max_w` caracteres de ancho.
pub fn wrap_text(text: &str, max_w: usize) -> Vec<String> {
    if max_w == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + max_w).min(chars.len());
        lines.push(chars[start..end].iter().collect());
        start = end;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Renderiza un panel completo (borde + título + contenido según modo).
///
/// Decide el formato por altura disponible, no por `mode`:
/// - height <= 2: línea colapsada `──[N]──Título────`
/// - height >= 3: borde + contenido (expanded/minimal)
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    kind: PanelKind,
    title: &str,
    items: &[String],
    selected_idx: usize,
    scroll_offset: usize,
    focused: bool,
    _mode: PanelMode,
) {
    let items_for_bar_len = if area.height <= 2 { 0 } else { items.len() };
    let selected_for_bar = if area.height <= 2 { 0 } else { selected_idx };

    if area.height <= 2 {
        render_collapsed_line(frame, area, title, focused);
    } else {
        render_expanded(frame, area, kind, title, items, selected_idx, scroll_offset, focused);
    }

    // Scrollbar pasivo para listas que lo necesiten
    if items_for_bar_len > 1 && area.height >= 3 {
        render_scrollbar(frame, area, items_for_bar_len, selected_for_bar);
    }
}

/// Línea compacta sin bordes: `──[1]──Tablas────────────────────────` (ancho completo)
fn render_collapsed_line(frame: &mut Frame<'_>, area: Rect, title: &str, focused: bool) {
    if area.width < 5 {
        return;
    }

    let fg = if focused { Color::Cyan } else { Color::Gray };
    let prefix = "─".to_string();
    #[allow(clippy::cast_possible_truncation)]
    let prefix_cols = prefix.chars().count() as u16;
    let max_title = area.width.saturating_sub(prefix_cols).max(1) as usize;
    let short_title: String = title.chars().take(max_title).collect();
    #[allow(clippy::cast_possible_truncation)]
    let used_cols = prefix_cols + short_title.chars().count() as u16;
    let padding_cols = area.width.saturating_sub(used_cols) as usize;
    let pad_str = "─".repeat(padding_cols);

    let line = RatLine::from(vec![
        Span::styled(prefix, Style::default().fg(fg)),
        Span::styled(short_title, Style::default().fg(fg)),
        Span::styled(pad_str, Style::default().fg(fg)),
    ]);

    let para = ratatui::widgets::Paragraph::new(line);
    frame.render_widget(para, area);
}

#[allow(clippy::too_many_arguments)]
fn render_expanded(
    frame: &mut Frame<'_>,
    area: Rect,
    kind: PanelKind,
    title: &str,
    items: &[String],
    selected_idx: usize,
    scroll_offset: usize,
    focused: bool,
) {
    let inner = inner_area_for_iteration(area);
    if inner.height == 0 {
        let block = panel_block(title, focused);
        frame.render_widget(block, area);
        return;
    }

    let viewport = usize::from(inner.height);

    // Auto-scroll suave: solo mueve 1 línea cuando la selección sale del viewport
    let scroll = if focused {
        if selected_idx >= scroll_offset.saturating_add(viewport) {
            scroll_offset.saturating_add(1)
        } else if selected_idx < scroll_offset {
            scroll_offset.saturating_sub(1)
        } else {
            scroll_offset
        }
    } else {
        scroll_offset
    };

    // Sources no enfocado: solo 1 ítem visible
    let max_visible = if kind == PanelKind::Sources && !focused { 1usize } else { viewport };

    let visible = items.iter().skip(scroll).take(max_visible);

    let list_items: Vec<ListItem<'_>> = visible
        .enumerate()
        .map(|(i, item)| {
            let global_idx = scroll + i;
            if global_idx == selected_idx {
                ListItem::new(format!("> {item}"))
            } else {
                ListItem::new(format!("  {item}"))
            }
        })
        .collect();

    let list = List::new(list_items)
        .block(panel_block(title, focused))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    let mut state = ListState::default().with_selected(if items.is_empty() {
        None
    } else {
        Some(selected_idx.saturating_sub(scroll))
    });
    frame.render_stateful_widget(list, area, &mut state);
}

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    let border_style =
        if focused { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::Gray) };

    Block::default().title(title.to_string()).borders(Borders::ALL).border_style(border_style)
}

fn render_scrollbar(frame: &mut Frame<'_>, area: Rect, content_len: usize, selected_idx: usize) {
    if area.height < 3 {
        return;
    }

    let viewport_len = usize::from(area.height.saturating_sub(2));
    let state = ScrollbarState::new(content_len)
        .viewport_content_length(viewport_len)
        .position(selected_idx.min(content_len.saturating_sub(1)));

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(ratatui::symbols::scrollbar::VERTICAL);
    let mut state_mut = state;
    frame.render_stateful_widget(scrollbar, area, &mut state_mut);
}

/// Área utilizable (sin bordes) para iterar items.
const fn inner_area_for_iteration(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}
