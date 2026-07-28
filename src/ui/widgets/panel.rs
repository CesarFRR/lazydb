/// Widget para renderizar un `Panel` de la UI.
///
/// Soporta los modos Collapsed (solo borde + título) y Expanded (contenido completo
/// con scroll). El modo Minimal está implementado pero no expuesto al usuario.
use ratatui::{
    prelude::*,
    text::Line as RatLine,
    text::Span,
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState,
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
) -> usize {
    let items_for_bar_len = if area.height <= 2 { 0 } else { items.len() };
    let selected_for_bar = if area.height <= 2 { 0 } else { selected_idx };

    let scroll = if area.height <= 2 {
        render_collapsed_line(frame, area, kind, title, focused);
        scroll_offset
    } else {
        render_expanded(frame, area, kind, title, items, selected_idx, scroll_offset, focused)
    };

    // Scrollbar pasivo para listas que lo necesiten
    if items_for_bar_len > 1 && area.height >= 3 {
        render_scrollbar(frame, area, items_for_bar_len, selected_for_bar);
    }

    scroll
}

/// Renderiza el panel Detail en modo Data como una tabla con columnas reales,
/// separadores verticales `│` y fila separadora `─┼─`.
///
/// `items[0]` = cabecera con nombres separados por ` | `
/// `items[1..]` = filas de datos separadas por ` | `
#[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::cast_possible_truncation)]
pub fn render_data_table(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: &[String],
    selected_idx: usize,
    scroll_offset: usize,
    focused: bool,
) -> usize {
    let inner = inner_area_for_iteration(area);
    if inner.height == 0 || items.is_empty() {
        let block = panel_block(title, focused);
        frame.render_widget(block, area);
        return scroll_offset;
    }

    let viewport = usize::from(inner.height);

    // Parsear columnas desde la cabecera (items[0])
    let headers: Vec<&str> = items[0].split(" | ").collect();
    let col_count = headers.len().max(1);

    if col_count == 1 {
        // Una sola columna: delegar al List normal (│ no tiene sentido)
        return render_expanded(frame, area, PanelKind::Detail, title, items, selected_idx, scroll_offset, focused);
    }

    // Ancho disponible para celdas (inner sin bordes)
    let inner_w = usize::from(inner.width);

    // Distribuir columnas equitativamente
    let cell_base = inner_w / col_count;
    let cell_widths: Vec<usize> = (0..col_count)
        .map(|i| {
            if i == col_count.saturating_sub(1) {
                inner_w.saturating_sub(cell_base * (col_count.saturating_sub(1)))
            } else {
                cell_base
            }
        })
        .collect();
    let widths: Vec<Constraint> =
        cell_widths.iter().map(|&w| Constraint::Length(w as u16)).collect();

    // ── Auto-scroll sobre datos (items[1..]) ──
    // Usamos salto directo (no animación lineal) para evitar que N filas nuevas
    // tarden N frames en mostrarse. Cuando la selección sale del viewport,
    // el scroll salta inmediatamente a la posición que la mantiene visible.
    let data_len = items.len().saturating_sub(1);
    let data_selected = if selected_idx == 0 { 0 } else { selected_idx.saturating_sub(1).min(data_len.saturating_sub(1)) };

    let vp_data = viewport.saturating_sub(3); // filas de datos visibles
    let max_scroll = data_len.saturating_sub(vp_data);

    let scroll = if focused && data_len > 0 && vp_data > 0 {
        if data_selected >= scroll_offset.saturating_add(vp_data) {
            // Selección salió por abajo → mostrar al final del viewport
            (data_selected.saturating_sub(vp_data).saturating_add(1)).min(max_scroll)
        } else if data_selected < scroll_offset {
            // Selección salió por arriba → mostrar al inicio del viewport
            data_selected
        } else {
            scroll_offset
        }
    } else {
        scroll_offset
    };

    // ── Construir filas ──
    let mut all_rows: Vec<Row<'_>> = Vec::new();

    // Fila 0: Espaciador entre los tabs (título del borde) y la tabla
    {
        let spacer: Vec<Cell<'_>> = cell_widths
            .iter()
            .map(|&w| Cell::from(" ".repeat(w)))
            .collect();
        all_rows.push(Row::new(spacer).height(1));
    }

    // Fila 1: Header (Bold + Cyan)
    let header_cells: Vec<Cell<'_>> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let w = cell_widths[i];
            let text = if i < col_count.saturating_sub(1) {
                let iw = w.saturating_sub(3);
                let val = truncate_middle(h.trim(), iw);
                format!(" {val:<iw$} │")
            } else {
                let iw = w.saturating_sub(1);
                let val = truncate_middle(h.trim(), iw);
                format!(" {val:<iw$}")
            };
            Cell::from(Span::styled(
                text,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
        })
        .collect::<Vec<Cell<'_>>>();
    all_rows.push(Row::new(header_cells).height(1));

    // Fila 2: Separador `─┼─` (referencia visual para alinear │)
    let sep_cells: Vec<Cell<'_>> = cell_widths
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let text = if i < col_count.saturating_sub(1) {
                let iw = w.saturating_sub(2);
                format!(" {}┼", "─".repeat(iw))
            } else {
                format!(" {}", "─".repeat(w.saturating_sub(1)))
            };
            Cell::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
        })
        .collect::<Vec<Cell<'_>>>();
    all_rows.push(Row::new(sep_cells).height(1));

    // Filas de datos (con scroll)
    let max_visible = viewport.saturating_sub(3); // spacer + header + separator
    let visible_data: Vec<&String> = items.iter().skip(1).skip(scroll).take(max_visible).collect();
    let visible_selected = if selected_idx > 0 {
        selected_idx.saturating_sub(1).saturating_sub(scroll)
    } else {
        usize::MAX // sin selección (header enfocado)
    };

    for (rel_idx, line) in visible_data.iter().enumerate() {
        let is_selected = rel_idx == visible_selected;
        let cells: Vec<&str> = line.split(" | ").collect();
        let row_cells: Vec<Cell<'_>> = (0..col_count)
            .map(|i| {
                let w = cell_widths[i];
                let val = cells.get(i).map_or("", |s| s.trim());
                #[allow(clippy::let_and_return)]
                if i == 0 {
                    // Primera columna con ▸ para selección
                    let iw = w.saturating_sub(3);
                    let truncated = truncate_middle(val, iw);
                    let prefix = if is_selected { "▸" } else { " " };
                    let text = format!("{prefix}{truncated:<iw$} │");
                    if is_selected {
                        Cell::from(Span::styled(text, Style::default().add_modifier(Modifier::BOLD)))
                    } else {
                        Cell::from(text)
                    }
                } else if i < col_count.saturating_sub(1) {
                    let iw = w.saturating_sub(3);
                    let truncated = truncate_middle(val, iw);
                    let text = format!(" {truncated:<iw$} │");
                    if is_selected {
                        Cell::from(Span::styled(text, Style::default().add_modifier(Modifier::BOLD)))
                    } else {
                        Cell::from(text)
                    }
                } else {
                    let iw = w.saturating_sub(1);
                    let truncated = truncate_middle(val, iw);
                    let text = format!(" {truncated:<iw$}");
                    if is_selected {
                        Cell::from(Span::styled(text, Style::default().add_modifier(Modifier::BOLD)))
                    } else {
                        Cell::from(text)
                    }
                }
            })
            .collect::<Vec<Cell<'_>>>();
        all_rows.push(Row::new(row_cells));
    }

    let table = Table::new(all_rows, widths)
        .block(panel_block(title, focused))
        .column_spacing(0);

    let mut state = TableState::default().with_selected(None);
    frame.render_stateful_widget(table, area, &mut state);

    scroll
}

/// Línea compacta sin bordes: `──[1]──Tablas────────────────────────` (ancho completo)
fn render_collapsed_line(
    frame: &mut Frame<'_>,
    area: Rect,
    kind: PanelKind,
    title: &str,
    focused: bool,
) {
    if area.width < 5 {
        return;
    }

    let fg = if focused { Color::Cyan } else { Color::Gray };
    let num = kind.number();
    let prefix = format!("─[{num}]─");
    // Quitar [N] del título si viene de title_for (para no duplicar)
    let clean_title =
        title.strip_prefix(&format!("[{num}]")).map_or_else(|| title.to_string(), str::to_string);
    #[allow(clippy::cast_possible_truncation)]
    let prefix_cols = prefix.chars().count() as u16;
    let max_title = area.width.saturating_sub(prefix_cols).max(1) as usize;
    let short_title: String = clean_title.chars().take(max_title).collect();
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
) -> usize {
    let inner = inner_area_for_iteration(area);
    if inner.height == 0 {
        let block = panel_block(title, focused);
        frame.render_widget(block, area);
        return scroll_offset;
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

    scroll
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
