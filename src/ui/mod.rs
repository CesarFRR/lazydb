pub mod layout;
pub mod widgets;

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::controller::{LayoutMode, SourceTab};
use crate::app::{App, PanelKind};

// ---------------------------------------------------------------------------
// Render principal
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();

    if area.height < 10 || area.width < 40 {
        render_too_small(frame, area);
        return;
    }

    // El layout ya fue computado en el loop principal
    render_header(frame, app.layout.header, app);
    render_footer(frame, app.layout.footer, app);

    // Renderizar cada panel en su posición
    for &(kind, rect) in &app.layout.panels {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        render_panel_at(frame, rect, kind, app);
    }

    // Menú de acciones (modal overlay)
    if app.show_actions_menu {
        render_actions_menu(frame, area, app);
    }
}

// ---------------------------------------------------------------------------
// Panel individual
// ---------------------------------------------------------------------------

fn render_panel_at(frame: &mut Frame<'_>, area: Rect, kind: PanelKind, app: &App) {
    let panel = app.panels.iter().find(|p| p.kind == kind).expect("panel not found");
    let title = app.title_for(kind);
    let items = app.items_for(kind);
    let focused = app.active_panel == kind;

    // Tabs especiales para Sources (All/Local/Online)
    let adjusted_area = if kind == PanelKind::Sources {
        let tab_area = Rect { height: 1, ..area };
        render_source_tabs(frame, tab_area, app);
        Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area }
    } else {
        area
    };

    widgets::panel::render(
        frame,
        adjusted_area,
        kind,
        &title,
        items,
        panel.selected_idx,
        panel.scroll_offset,
        focused,
        panel.mode,
    );
}

// ---------------------------------------------------------------------------
// Source tabs (All / Local / Online)
// ---------------------------------------------------------------------------

fn render_source_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if area.width < 20 {
        return;
    }
    let tabs_text = match app.source_tab {
        SourceTab::All => "[Todo] Local Online",
        SourceTab::Local => "Todo [Local] Online",
        SourceTab::Online => "Todo Local [Online]",
    };
    let para = Paragraph::new(tabs_text).style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let layout_mode = LayoutMode::from_width(area.width);

    let query_indicator = match &app.query_state {
        crate::query::QueryState::Idle => String::new(),
        crate::query::QueryState::Running => " | [Ejecutando query...]".to_string(),
        crate::query::QueryState::Done(_) => " | [Query completada]".to_string(),
        crate::query::QueryState::Error(e) => format!(" | [Error: {e}]"),
    };

    let line1 = format!(
        "lazydb | foco: {} | layout: {} | db: {} ({}){}",
        app.active_panel.label(),
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

    if app.layout.compact_height {
        frame.render_widget(Paragraph::new(fit_line(&line1, area.width)), area);
        return;
    }

    let header_text = format!("{}\n{}", fit_line(&line1, area.width), fit_line(&line2, area.width));
    frame.render_widget(Paragraph::new(header_text), area);
}

// ---------------------------------------------------------------------------
// Footer / status bar
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if area.width >= 110 {
        let shortcuts = format!(
            "tab: foco paneles | ↑↓: seleccion | ←→: tabs detalle | space: toggle | 1-5: ir panel | rueda: scroll | click: foco/item | x: menu | ctrl+q: count | {}",
            app.status
        );
        frame.render_widget(Paragraph::new(shortcuts), area);
    } else {
        let shortcuts = format!(
            "tab foco | ↑↓ mover | ←→ detalle | space toggle | rueda | x menu | {}",
            app.status
        );
        frame.render_widget(Paragraph::new(shortcuts), area);
    }
}

// ---------------------------------------------------------------------------
// Actions menu (modal overlay)
// ---------------------------------------------------------------------------

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

    let border_style = Style::default().fg(Color::Cyan);
    let block = Block::default()
        .title("Acciones (x/b cerrar, Enter ejecutar)")
        .borders(Borders::ALL)
        .border_style(border_style);

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);
}

// ---------------------------------------------------------------------------
// Terminal muy pequeña
// ---------------------------------------------------------------------------

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
