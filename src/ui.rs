use ratatui::{
    prelude::*,
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

use crate::app::{App, DetailTab, FocusPanel, LayoutMode, ObjectSection};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();

    if area.height < 10 || area.width < 40 {
        render_too_small(frame, area);
        return;
    }

    let layout_mode = LayoutMode::from_width(area.width);
    let compact_height = area.height < 14;

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
        LayoutMode::Large | LayoutMode::Medium => render_two_columns(frame, vertical[1], app),
        LayoutMode::Small => render_small(frame, vertical[1], app),
    }

    if app.show_actions_menu {
        render_actions_menu(frame, area, app);
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
    let query_indicator = match &app.query_state {
        crate::query::QueryState::Idle => String::new(),
        crate::query::QueryState::Running => " | [Ejecutando query...]".to_string(),
        crate::query::QueryState::Done(_) => " | [Query completada]".to_string(),
        crate::query::QueryState::Error(e) => format!(" | [Error: {e}]"),
    };

    let line1 = format!(
        "lazydb | foco: {} | layout: {} | db: {} ({}){}",
        app.focus.title(),
        layout_mode.label(),
        app.db_path_display(),
        app.db_size_display(),
        query_indicator
    );

    let line2 = format!(
        "src:{} | obj:{} | detail:{} | selected:{}",
        app.source_tab_label(),
        app.object_section_label(),
        app.detail_tab_label(),
        app.selected_object()
    );

    if compact_height {
        frame.render_widget(Paragraph::new(fit_line(&line1, area.width)), area);
        return;
    }

    let header_text = format!("{}\n{}", fit_line(&line1, area.width), fit_line(&line2, area.width));
    frame.render_widget(Paragraph::new(header_text), area);
}

fn render_two_columns(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(67)])
        .split(area);

    render_left_navigation(frame, cols[0], app);
    render_right_detail(frame, cols[1], app);
}

fn render_small(frame: &mut Frame<'_>, area: Rect, app: &App) {
    match app.focus {
        FocusPanel::Sources | FocusPanel::Objects => render_left_navigation(frame, area, app),
        FocusPanel::Preview => render_right_detail(frame, area, app),
    }
}

fn render_left_navigation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(area);

    render_sources(frame, rows[0], app);
    render_object_section(frame, rows[1], app, ObjectSection::Tables);
    render_object_section(frame, rows[2], app, ObjectSection::Views);
    render_object_section(frame, rows[3], app, ObjectSection::Advanced);
}

fn render_sources(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tabs = match app.source_tab {
        crate::app::SourceTab::All => "[Todo] Local Online",
        crate::app::SourceTab::Local => "Todo [Local] Online",
        crate::app::SourceTab::Online => "Todo Local [Online]",
    };
    let title = format!("Fuentes ({tabs})");

    let items: Vec<ListItem<'_>> =
        app.sources.iter().map(|item| ListItem::new(item.as_str())).collect();

    let mut state = ListState::default().with_selected(Some(app.source_idx));
    let list = List::new(items)
        .block(panel_block(&title, app.focus == FocusPanel::Sources))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
    render_passive_scrollbar(frame, area, app.sources.len(), app.source_idx);
}

fn render_object_section(frame: &mut Frame<'_>, area: Rect, app: &App, section: ObjectSection) {
    let (items_vec, title) = match section {
        ObjectSection::Tables => (&app.tables, "Tablas"),
        ObjectSection::Views => (&app.views, "Vistas"),
        ObjectSection::Advanced => (&app.advanced, "Avanzado"),
    };

    let items: Vec<ListItem<'_>> = if items_vec.is_empty() {
        vec![ListItem::new("<sin objetos>")]
    } else {
        items_vec.iter().map(|item| ListItem::new(item.as_str())).collect()
    };

    let selected = if app.object_section == section {
        Some(app.object_idx.min(items.len().saturating_sub(1)))
    } else {
        None
    };

    let mut state = ListState::default().with_selected(selected);
    let list = List::new(items)
        .block(panel_block(
            title,
            app.focus == FocusPanel::Objects && app.object_section == section,
        ))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
    let selected_for_bar = if app.object_section == section { app.object_idx } else { 0 };
    render_passive_scrollbar(frame, area, items_vec.len().max(1), selected_for_bar);
}

fn render_right_detail(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3)])
        .split(area);

    render_detail_tabs(frame, rows[0], app);
    render_detail_content(frame, rows[1], app);
}

fn render_detail_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tabs = [DetailTab::Data, DetailTab::Schema, DetailTab::Sql, DetailTab::Meta]
        .iter()
        .map(|tab| {
            if *tab == app.detail_tab {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");

    let title = "Detalle [Datos | Esquema | SQL | Meta] (←/→ cambia)";
    let paragraph = Paragraph::new(fit_line(&tabs, area.width))
        .block(panel_block(title, app.focus == FocusPanel::Preview));

    frame.render_widget(paragraph, area);
}

fn render_detail_content(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let title = if app.detail_tab == DetailTab::Data {
        let total_pages =
            if app.total_rows == 0 { 1 } else { app.total_rows.div_ceil(app.rows_per_page) };
        format!(
            "{} | Page {}/{} | Row {}/{}",
            app.detail_tab_label(),
            app.current_page + 1,
            total_pages,
            app.preview_idx + 1,
            app.preview_rows.len()
        )
    } else {
        app.detail_tab_label().to_string()
    };

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

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    Block::default().title(title).borders(Borders::ALL).border_style(border_style)
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let shortcuts = if area.width >= 110 {
        format!(
            "tab: tabs izq | ↑/↓: contenido | ←/→: tabs detalle | rueda: scroll | click: foco/seleccion | x/b: menu acciones | enter abrir | ctrl+q count | ctrl+r cfg | {}",
            app.status
        )
    } else {
        format!("tab tabs izq | ↑↓ mover | ←→ detalle | rueda | x menu | {}", app.status)
    };
    frame.render_widget(Paragraph::new(shortcuts), area);
}

fn render_actions_menu(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let width = area.width.min(52);
    let height = area.height.min(10);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    frame.render_widget(Clear, rect);

    let lines = App::actions_menu_items()
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            if idx == app.actions_menu_selected() {
                format!("> {item}")
            } else {
                format!("  {item}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let paragraph = Paragraph::new(lines)
        .block(panel_block("Acciones (x/b cerrar, Enter ejecutar)", true))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);
}

fn render_passive_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_len: usize,
    selected_idx: usize,
) {
    if area.height < 3 || content_len <= 1 {
        return;
    }

    let viewport_len = usize::from(area.height.saturating_sub(2));
    let mut state = ScrollbarState::new(content_len)
        .viewport_content_length(viewport_len)
        .position(selected_idx.min(content_len.saturating_sub(1)));

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(ratatui::symbols::scrollbar::VERTICAL);
    frame.render_stateful_widget(scrollbar, area, &mut state);
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
