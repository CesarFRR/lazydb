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

/// Renderiza un panel completo (borde + título + contenido según modo).
///
/// `items`: lista de strings a mostrar (vacío para Collapsed).
/// `selected_idx`: índice seleccionado.
/// `scroll_offset`: primera fila visible.
/// `focused`: si el panel tiene el foco (borde cyan).
/// `mode`: modo de renderizado.
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
    mode: PanelMode,
) {
    let items_for_bar_len = match mode {
        PanelMode::Collapsed => 0,
        _ => items.len(),
    };
    let selected_for_bar = match mode {
        PanelMode::Collapsed => 0,
        _ => selected_idx,
    };

    match mode {
        PanelMode::Collapsed => {
            render_collapsed_line(frame, area, title, items.len(), focused);
        }
        PanelMode::Minimal | PanelMode::Expanded | PanelMode::Fixed(_) => {
            render_expanded(
                frame,
                area,
                title,
                items,
                selected_idx,
                scroll_offset,
                focused,
                kind == PanelKind::Detail,
            );
        }
    }

    // Scrollbar pasivo para listas que lo necesiten
    if items_for_bar_len > 1 && area.height >= 3 {
        render_scrollbar(frame, area, items_for_bar_len, selected_for_bar);
    }
}

/// Línea compacta sin bordes: `──[N]──Título────────────────`
#[allow(clippy::too_many_arguments)]
fn render_collapsed_line(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    count: usize,
    focused: bool,
) {
    if area.width < 5 {
        return;
    }

    let fg = if focused { Color::Cyan } else { Color::Gray };
    let count_str = format!("[{count}]");
    // Truncar título si es muy largo
    #[allow(clippy::cast_possible_truncation)]
    let max_title = area.width.saturating_sub(count_str.len() as u16 + 6).max(1) as usize;
    let short_title: String = title.chars().take(max_title).collect();

    let line = RatLine::from(vec![
        Span::styled("──", Style::default().fg(fg)),
        Span::styled(&count_str, Style::default().fg(fg)),
        Span::styled("──", Style::default().fg(fg)),
        Span::styled(&short_title, Style::default().fg(fg)),
    ]);

    let para = ratatui::widgets::Paragraph::new(line);
    frame.render_widget(para, area);
}

#[allow(clippy::too_many_arguments)]
fn render_expanded(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: &[String],
    selected_idx: usize,
    scroll_offset: usize,
    focused: bool,
    _is_detail: bool,
) {
    let inner = inner_area_for_iteration(area);
    if inner.height == 0 {
        // Área demasiado pequeña: solo borde
        let block = panel_block(title, focused);
        frame.render_widget(block, area);
        return;
    }

    let viewport = usize::from(inner.height);
    let visible = items.iter().skip(scroll_offset).take(viewport);

    let list_items: Vec<ListItem<'_>> = visible
        .enumerate()
        .map(|(i, item)| {
            let global_idx = scroll_offset + i;
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
        Some(selected_idx.saturating_sub(scroll_offset))
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
