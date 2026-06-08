pub mod layout;
pub mod widgets;

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{App, PanelKind};

// ---------------------------------------------------------------------------
// Render principal
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();

    if area.height < 3 || area.width < 12 {
        render_too_small(frame, area);
        return;
    }

    // El layout ya fue computado en el loop principal
    render_footer(frame, app.layout.footer, app);

    // Renderizar cada panel en su posición
    for &(kind, rect) in &app.layout.panels {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        render_panel_at(frame, rect, kind, app);
    }

    // Inspector de fila (modal tabla con word-wrap)
    if app.show_row_inspector {
        widgets::modal::render_table(
            frame,
            area,
            &format!("▸ {}", app.selected_object()),
            &app.row_inspector_pairs,
            &app.inspector_scroll,
            70,
            70,
        );
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

    widgets::panel::render(
        frame,
        area,
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
// Footer / status bar
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if area.width >= 110 {
        let shortcuts = format!(
            "tab: foco paneles | ↑↓: seleccion | ←→: mover sidebar | []: tabs detalle | space: toggle | 1-5: ir panel | rueda: scroll | x: menu | ctrl+q: count | {}",
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
    frame.render_widget(Paragraph::new(msg), area);
}
