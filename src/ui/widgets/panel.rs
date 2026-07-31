/// Widget para renderizar un `Panel` de la UI.
///
/// Soporta los modos Collapsed (solo borde + título) y Expanded (contenido completo
/// con scroll). El modo Minimal está implementado pero no expuesto al usuario.
use ratatui::{
    prelude::*,
    text::Line as RatLine,
    text::Span,
    widgets::{Block, Borders, Cell, List, ListItem, ListState, Row, Table, TableState},
};

use crate::app::{PanelKind, PanelMode};

/// Trunca un string con un carácter de elipsis en el medio si excede `max_w`.
/// Ej: "Luis Hernando Garcia..." → "Luis Hern…arcia"
pub fn truncate_middle(text: &str, max_w: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_w {
        return text.to_string();
    }
    if max_w < 5 {
        return chars.iter().take(max_w).collect::<String>();
    }
    let half = (max_w - 1) / 2;
    let left: String = chars.iter().take(half).collect();
    let right: String = chars.iter().rev().take(half).collect::<String>().chars().rev().collect(); // revert the reversed collect
    format!("{left}…{right}")
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
    let scroll = if area.height <= 2 {
        render_collapsed_line(frame, area, kind, title, focused);
        scroll_offset
    } else {
        render_expanded(frame, area, kind, title, items, selected_idx, scroll_offset, focused)
    };

    // Scrollbar pasivo para listas que lo necesiten.
    // Modelo "selección" (estilo lazygit): el thumb refleja la posición del
    // ítem SELECCIONADO dentro de la lista, no la del viewport — así al
    // arrastrar la barra hasta el fondo el último ítem queda seleccionado
    // (y el drag con mouse ya sincroniza selected_idx = scroll).
    if items.len() > 1 && area.height >= 3 {
        draw_v_scrollbar(frame, area, items.len(), selected_idx);
    }

    scroll
}

/// Renderiza el panel Detail en modo Data como una tabla con columnas reales,
/// separadores verticales `│` y fila separadora `─┼─`.
///
/// `items[0]` = cabecera con nombres separados por ` | `
/// `items[1..]` = filas de datos separadas por ` | `
/// Ancho mínimo de columna en el Data tab. Si todas las columnas caben
/// con este ancho, se distribuyen equitativamente; si no, se activa
/// scroll horizontal (shift+rueda / ctrl+rueda).
pub const MIN_COL_W: usize = 12;

#[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::cast_possible_truncation)]
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn render_data_table(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: &[String],
    selected_idx: usize,
    scroll_offset: usize,
    h_scroll: usize,
    focused: bool,
    sort_column: Option<&str>,
    sort_asc: bool,
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
        return render_expanded(
            frame,
            area,
            PanelKind::Detail,
            title,
            items,
            selected_idx,
            scroll_offset,
            focused,
        );
    }

    // Ancho disponible para celdas (inner sin bordes)
    let inner_w = usize::from(inner.width);

    // ── Ventana de columnas visibles según h_scroll ──
    // Si todas las columnas caben con ancho mínimo → distribuir equitativamente.
    // Si no → ancho fijo MIN_COL_W y h_scroll elige la primera visible.
    let total_min = col_count.saturating_mul(MIN_COL_W);
    let (vis_start, vis_end, cell_widths) = if total_min <= inner_w {
        let cell_base = inner_w / col_count;
        let widths: Vec<usize> = (0..col_count)
            .map(|i| {
                if i == col_count.saturating_sub(1) {
                    inner_w.saturating_sub(cell_base * (col_count.saturating_sub(1)))
                } else {
                    cell_base
                }
            })
            .collect();
        (0, col_count, widths)
    } else {
        let max_visible = (inner_w / MIN_COL_W).max(1);
        let vis_start = h_scroll.min(col_count.saturating_sub(max_visible));
        let vis_end = vis_start + max_visible;
        let mut widths = vec![MIN_COL_W; max_visible];
        let rem = inner_w.saturating_sub(max_visible.saturating_mul(MIN_COL_W));
        if let Some(last) = widths.last_mut() {
            *last += rem;
        }
        (vis_start, vis_end, widths)
    };
    let _vis_count = vis_end.saturating_sub(vis_start);
    let widths: Vec<Constraint> =
        cell_widths.iter().map(|&w| Constraint::Length(w as u16)).collect();

    // Ajustar título si hay scroll horizontal activo
    let has_h_scroll = total_min > inner_w;
    let title = if has_h_scroll {
        format!("{title} — cols {}-{}/{}", vis_start + 1, vis_end, col_count)
    } else {
        title.to_string()
    };

    // ── Auto-scroll sobre datos (items[1..]) ──
    // Usamos salto directo (no animación lineal) para evitar que N filas nuevas
    // tarden N frames en mostrarse. Cuando la selección sale del viewport,
    // el scroll salta inmediatamente a la posición que la mantiene visible.
    let data_len = items.len().saturating_sub(1);
    let data_selected = if selected_idx == 0 {
        0
    } else {
        selected_idx.saturating_sub(1).min(data_len.saturating_sub(1))
    };

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
        let spacer: Vec<Cell<'_>> =
            cell_widths.iter().map(|&w| Cell::from(" ".repeat(w))).collect();
        all_rows.push(Row::new(spacer).height(1));
    }

    // Fila 1: Header (Bold + Cyan) con indicador de orden ▴/▾
    // El `iw` base es siempre w-3 (como filas de datos), el indicador se
    // inserta dentro del padding derecho SIN robarle espacio al nombre.
    let header_cells: Vec<Cell<'_>> = (vis_start..vis_end)
        .map(|i| {
            let w = cell_widths[i - vis_start];
            let h_trimmed = headers[i].trim();
            let has_indicator = sort_column == Some(h_trimmed);
            let is_last = i == vis_end.saturating_sub(1);
            let text = if is_last {
                let iw = w.saturating_sub(1);
                let val = truncate_middle(h_trimmed, iw);
                // Última columna visible: sin separador │, el indicador va pegado al nombre
                if has_indicator {
                    let ch = if sort_asc { '▴' } else { '▾' };
                    let padded = format!("{val:<iw$}");
                    let last_space = padded.rfind(' ').unwrap_or_else(|| iw.saturating_sub(1));
                    let mut chars: Vec<char> = padded.chars().collect();
                    if last_space < chars.len() {
                        chars[last_space] = ch;
                    }
                    format!(" {}", chars.iter().collect::<String>())
                } else {
                    format!(" {val:<iw$}")
                }
            } else {
                let iw = w.saturating_sub(3);
                let val = truncate_middle(h_trimmed, iw);
                // El ▴/▾ reemplaza el último espacio del padding antes de " │"
                let padded = format!("{val:<iw$}");
                if has_indicator {
                    let ch = if sort_asc { '▴' } else { '▾' };
                    // Reemplazar el último espacio (o último char si no hay espacio)
                    let last_space = padded.rfind(' ').unwrap_or_else(|| iw.saturating_sub(1));
                    let mut chars: Vec<char> = padded.chars().collect();
                    if last_space < chars.len() {
                        chars[last_space] = ch;
                    }
                    format!(" {} │", chars.iter().collect::<String>())
                } else {
                    format!(" {padded} │")
                }
            };
            Cell::from(Span::styled(
                text,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))
        })
        .collect::<Vec<Cell<'_>>>();
    all_rows.push(Row::new(header_cells).height(1));

    // Fila 2: Separador `─┼─` (referencia visual para alinear │)
    let sep_cells: Vec<Cell<'_>> = (vis_start..vis_end)
        .map(|i| {
            let w = cell_widths[i - vis_start];
            let text = if i < vis_end.saturating_sub(1) {
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
        let row_cells: Vec<Cell<'_>> = (vis_start..vis_end)
            .map(|i| {
                let w = cell_widths[i - vis_start];
                let val = cells.get(i).map_or("", |s| s.trim());
                let is_first = i == vis_start;
                #[allow(clippy::let_and_return)]
                if is_first {
                    // Primera columna visible con ▸ para selección
                    let iw = w.saturating_sub(3);
                    let truncated = truncate_middle(val, iw);
                    let prefix = if is_selected { "▸" } else { " " };
                    let text = format!("{prefix}{truncated:<iw$} │");
                    if is_selected {
                        Cell::from(Span::styled(
                            text,
                            Style::default().add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Cell::from(text)
                    }
                } else if i < vis_end.saturating_sub(1) {
                    let iw = w.saturating_sub(3);
                    let truncated = truncate_middle(val, iw);
                    let text = format!(" {truncated:<iw$} │");
                    if is_selected {
                        Cell::from(Span::styled(
                            text,
                            Style::default().add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Cell::from(text)
                    }
                } else {
                    let iw = w.saturating_sub(1);
                    let truncated = truncate_middle(val, iw);
                    let text = format!(" {truncated:<iw$}");
                    if is_selected {
                        Cell::from(Span::styled(
                            text,
                            Style::default().add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        Cell::from(text)
                    }
                }
            })
            .collect::<Vec<Cell<'_>>>();
        all_rows.push(Row::new(row_cells));
    }

    let table = Table::new(all_rows, widths).block(panel_block(&title, focused)).column_spacing(0);

    let mut state = TableState::default().with_selected(None);
    frame.render_stateful_widget(table, area, &mut state);

    // ── Barra de scroll horizontal ──
    // SOLO si hay columnas ocultas (scroll horizontal activo). Ocupa la fila
    // del espaciador (entre el título con tabs y los headers), como overlay
    // después del Table.
    //
    // Se dibuja A MANO (sin ScrollbarState) porque la fórmula de ratatui
    // (`thumb_start = pos * track / (content - 1 + viewport)`) asume scroll
    // continuo de líneas y NUNCA lleva el thumb al final del track en scroll
    // discreto por columnas (con 20 cols y 8 visibles se quedaba en el 44%).
    // Esta fórmula es EXACTAMENTE la misma que usa el drag en el controller
    // (thumb proporcional a columnas visibles, track = inner_w - thumb_w), así
    // el thumb visual coincide 1:1 con la posición del scroll.
    if has_h_scroll {
        let max_visible = (inner_w / MIN_COL_W).max(1);
        let max_start = col_count.saturating_sub(max_visible);
        let thumb_w =
            (inner_w as f32 * max_visible as f32 / col_count as f32).round().max(1.0) as usize;
        let track = inner_w.saturating_sub(thumb_w).max(1);
        // División entera: con vis_start == max_start el thumb toca el borde
        let thumb_start = vis_start.saturating_mul(track).checked_div(max_start).unwrap_or(0);
        // Thumb `▀` (mitad SUPERIOR, mismo grosor que ▄): al quedar pegado
        // arriba, la mitad vacía de la celda actúa como GAP sutil entre la
        // barra y los headers de la tabla — así las tres zonas interactivas
        // se distinguen: pestañas (borde del título), barra horizontal (esta
        // fila) y headers (click = ordenar). Con `▄` el thumb se pegaba a los
        // headers y la mitad vacía parecía "barra fantasma".
        let thumb_style = Style::default().fg(Color::Cyan);
        let buf = frame.buffer_mut();
        for i in thumb_start..thumb_start + thumb_w {
            let cell = buf
                .cell_mut((inner.x + i as u16, inner.y))
                .expect("celda dentro del área del panel");
            cell.set_symbol("▀").set_style(thumb_style);
        }
    }

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
        .map(|item| {
            // SIN prefijos hardcodeados: ratatui reserva la primera columna
            // para el highlight_symbol "▸ " (o su espacio en blanco) en TODAS
            // las filas, y pinta "▸" solo en la seleccionada. Así el texto de
            // todos los items queda alineado y el ▸ no empuja nada.
            ListItem::new(item.clone())
        })
        .collect();

    let list = List::new(list_items)
        .block(panel_block(title, focused))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");

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

/// Dibuja el scrollbar vertical de un panel A MANO (misma lógica que la barra
/// horizontal del Data tab), en la última columna del área, sobre las filas
/// internas (sin tocar esquinas ni el título del borde).
///
/// - thumb de largo FIJO: `round(inner_h * viewport / content_len)` (mín 1) —
///   nunca cambia con la posición (el de ratatui "respiraba" con el redondeo
///   de inicio/fin y parecía un gusano viviente).
/// - `track = inner_h - thumb_h` y `thumb_start = offset * track / max_scroll`:
///   con `offset == max_scroll` el thumb toca el borde inferior (recorre el 100%).
/// - Símbolos y estilos idénticos a la barra horizontal: `│` `DarkGray` / `█` `Cyan`.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub fn draw_v_scrollbar(frame: &mut Frame<'_>, area: Rect, content_len: usize, offset: usize) {
    if area.height < 3 || content_len <= 1 {
        return;
    }
    let inner_h = usize::from(area.height.saturating_sub(2));
    let viewport = inner_h;
    if content_len <= viewport {
        return; // sin scrollbar visible
    }
    let max_scroll = content_len.saturating_sub(viewport);
    let thumb_h = (inner_h as f32 * viewport as f32 / content_len as f32).round().max(1.0) as usize;
    let track = inner_h.saturating_sub(thumb_h).max(1);
    // División entera: con offset == max_scroll el thumb toca el borde inferior
    let thumb_start = offset.min(max_scroll).saturating_mul(track) / max_scroll;

    let x = area.x + area.width - 1;
    let track_style = Style::default().fg(Color::DarkGray);
    let thumb_style = Style::default().fg(Color::Cyan);
    let buf = frame.buffer_mut();
    for i in 0..inner_h {
        let (symbol, style) = if i >= thumb_start && i < thumb_start + thumb_h {
            ("█", thumb_style)
        } else {
            ("│", track_style)
        };
        let cell = buf.cell_mut((x, area.y + 1 + i as u16)).expect("celda del scrollbar");
        cell.set_symbol(symbol).set_style(style);
    }
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
