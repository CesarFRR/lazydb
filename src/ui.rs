use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, FocusPanel, LayoutMode};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();

    if area.height < 6 || area.width < 28 {
        render_too_small(frame, area);
        return;
    }

    let layout_mode = LayoutMode::from_width(area.width);
    let compact_height = area.height < 11;

    let header_height = if compact_height { 1 } else { 2 };
    let footer_height = 1;

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(area);

    render_header(frame, vertical[0], app, layout_mode, compact_height);

    match layout_mode {
        LayoutMode::Large => render_large(frame, vertical[1], app),
        LayoutMode::Medium => render_medium(frame, vertical[1], app),
        LayoutMode::Small => render_small(frame, vertical[1], app),
    }

    render_footer(frame, vertical[2], app);
}

fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    layout_mode: LayoutMode,
    compact_height: bool,
) {
    let line1 = format!(
        "lazydb | foco: {} | layout: {} | refresh: {}",
        app.focus.title(),
        layout_mode.label(),
        app.refresh_count
    );

    let line2 = format!(
        "source: {} | object: {} | row: {}",
        app.selected_source(),
        app.selected_object(),
        app.selected_preview_row()
    );

    if compact_height {
        frame.render_widget(Paragraph::new(fit_line(&line1, area.width)), area);
        return;
    }

    let header_text = format!("{}\n{}", fit_line(&line1, area.width), fit_line(&line2, area.width));
    frame.render_widget(Paragraph::new(header_text), area);
}

fn render_large(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(40),
        ])
        .split(area);

    render_sources(frame, cols[0], app, false);
    render_objects(frame, cols[1], app, false);
    render_preview(frame, cols[2], app, false);
}

fn render_medium(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_sources(frame, cols[0], app, false);

    match app.focus {
        FocusPanel::Sources => render_objects(frame, cols[1], app, false),
        FocusPanel::Objects | FocusPanel::Preview => render_preview(frame, cols[1], app, false),
    }
}

fn render_small(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.focus {
        FocusPanel::Sources => render_sources(frame, area, app, true),
        FocusPanel::Objects => render_objects(frame, area, app, true),
        FocusPanel::Preview => render_preview(frame, area, app, true),
    }
}

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Block::default().title(title).borders(Borders::ALL).border_style(border_style)
}

fn render_sources(frame: &mut Frame<'_>, area: Rect, app: &App, compact: bool) {
    let title = if compact { "Sources (solo)" } else { "Sources" };

    let items: Vec<ListItem<'_>> =
        app.sources.iter().map(|item| ListItem::new(item.as_str())).collect();
    let mut state = ListState::default().with_selected(Some(app.source_idx));
    let list = List::new(items)
        .block(panel_block(title, app.focus == FocusPanel::Sources))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_objects(frame: &mut Frame<'_>, area: Rect, app: &App, compact: bool) {
    let title = if compact { "Objects (solo)" } else { "Objects" };

    let items: Vec<ListItem<'_>> =
        app.objects.iter().map(|item| ListItem::new(item.as_str())).collect();
    let mut state = ListState::default().with_selected(Some(app.object_idx));
    let list = List::new(items)
        .block(panel_block(title, app.focus == FocusPanel::Objects))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_preview(frame: &mut Frame<'_>, area: Rect, app: &App, compact: bool) {
    let total_pages =
        if app.total_rows == 0 { 1 } else { app.total_rows.div_ceil(app.rows_per_page) };

    let page_info = format!(
        "Page {}/{} | Row {}/{}",
        app.current_page + 1,
        total_pages,
        app.preview_idx + 1,
        app.preview_rows.len()
    );

    let title = if compact { format!("{page_info} | solo") } else { page_info };

    let content = app
        .preview_rows
        .iter()
        .enumerate()
        .map(
            |(idx, row)| {
                if idx == app.preview_idx { format!("> {row}") } else { format!("  {row}") }
            },
        )
        .collect::<Vec<String>>()
        .join("\n");

    let paragraph = Paragraph::new(content)
        .block(panel_block(&title, app.focus == FocusPanel::Preview))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let shortcuts = if area.width >= 95 {
        format!(
            "tab/h/l foco | j/k mover | pgup/pgdn página | enter abrir | 1/2/3 panel | r refresh | q salir | {}",
            app.status
        )
    } else {
        format!("enter abrir | tab foco | pgup/pgdn página | q salir | {}", app.status)
    };
    frame.render_widget(Paragraph::new(shortcuts), area);
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect) {
    let msg = "Terminal pequena: amplia ancho/alto para ver lazydb";
    frame.render_widget(Paragraph::new(fit_line(msg, area.width)), area);
}

fn fit_line(input: &str, width: u16) -> String {
    let max = usize::from(width.saturating_sub(1));
    if input.chars().count() <= max {
        return input.to_owned();
    }

    input.chars().take(max).collect()
}
