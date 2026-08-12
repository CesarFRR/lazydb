pub mod layout;
pub mod theme;
pub mod widgets;

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::controller::DetailTab;
use crate::app::{App, PanelKind};
use crate::query::QueryState;

/// Spinner ASCII/Unicode para operaciones en segundo plano (patrón lazy:
/// la UI jamás se congela, y el estado en curso siempre es visible).
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ---------------------------------------------------------------------------
// Render principal
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();

    if area.height < 3 || area.width < 12 {
        render_too_small(frame, area);
        return;
    }

    // Resize race: si la terminal cambió entre `compute_layout(size)` y
    // `draw()` (el frame tiene el tamaño REAL del buffer), recomputar el
    // layout AHORA para que nunca sobresalga. Los clamps de abajo quedan
    // como red de seguridad para cualquier desfase residual.
    if app.layout.footer.width != area.width || app.layout.footer.height != area.height {
        app.compute_layout(area.width, area.height);
    }

    // El layout ya fue computado (y re-computado si hizo falta). Clamp
    // contra el frame por seguridad: ratatui 0.29 PANICA con rects que
    // sobresalen del buffer.
    let footer = app.layout.footer.intersection(area);
    if footer.height > 0 && footer.width > 0 {
        render_footer(frame, footer, app);
    }

    // Renderizar cada panel en su posición. Clamp contra el área REAL del
    // frame: al redimensionar la terminal, el layout (computado con el
    // tamaño anterior) puede sobresalir del buffer → ratatui paniquea
    // "index outside of buffer". `intersection` recorta sin panic.
    for &(kind, rect) in &app.layout.panels {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let rect = rect.intersection(area);
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        render_panel_at(frame, rect, kind, app);
    }

    // Inspector de fila (modal tabla con word-wrap)
    if app.show_row_inspector {
        // NoSQL: botón de modo en el título (`[J: json]` / `[J: pares]`).
        // El mismo texto es la zona clicable (ver `on_mouse_click`).
        let title = if app.is_nosql {
            format!("▸ {}   [Shift+J: {}]", app.selected_object(), app.inspector_mode_label())
        } else {
            format!("▸ {}", app.selected_object())
        };
        if app.inspector_json_mode {
            // NoSQL en modo JSON: el documento completo formateado.
            let lines: Vec<ratatui::text::Line<'_>> =
                app.inspector_json_text.lines().map(ratatui::text::Line::from).collect();
            widgets::modal::render_lines(
                frame,
                area,
                &title,
                &lines,
                &mut app.inspector_scroll,
                70,
                70,
                None,
            );
        } else {
            widgets::modal::render_table(
                frame,
                area,
                &title,
                &app.row_inspector_pairs,
                &mut app.inspector_scroll,
                70,
                70,
            );
        }
    }

    // Menú de acciones (modal overlay)
    if app.show_actions_menu {
        render_actions_menu(frame, area, app);
    }

    // Ayuda de teclas (modal overlay, encima de todo)
    if app.show_help {
        render_help(frame, area, app);
    }

    // Input SQL (modal `:` — buffer con cursor + historial estilo fish)
    if app.query.query_input.is_some() {
        render_query_input(frame, area, app);
    }

    // Prompt de contraseña (servidor detectado)
    if app.connection.password_prompt.is_some() {
        render_password_prompt(frame, area, app);
    }

    // Pick de base de datos (servidor detectado: SHOW DATABASES)
    if app.connection.db_picker.is_some() {
        render_db_picker(frame, area, app);
    }

    // Popup de error global (modal rojo, encima de todo — Enter/Esc/q cierra)
    if let Some(err) = &app.error {
        let title = format!(" ✗ {}", err.title);
        let mut scratch = crate::ui::widgets::modal::ModalScroll::default();
        widgets::modal::render_lines(
            frame,
            area,
            &title,
            &[Line::from(err.body.as_str())],
            &mut scratch,
            70,
            40,
            Some(Style::default().fg(crate::ui::theme::THEME.error)),
        );
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

    // Formulario de nueva conexión: REGLA DE ORO — sin db abierta, el Detail
    // SIEMPRE muestra el formulario, sin importar el panel enfocado ni Esc.
    if kind == PanelKind::Detail && app.db_path.is_none() {
        widgets::connection_form::render(frame, area, app);
        return;
    }

    // Tabla de datos con columnas reales para Detail + Data tab
    let new_scroll = if kind == PanelKind::Detail && app.data_view.detail_tab == DetailTab::Data {
        widgets::panel::render_data_table(
            frame,
            area,
            &title,
            app.data_view.preview_data.as_ref(),
            items,
            panel.selected_idx,
            panel.scroll_offset.get(),
            panel.h_scroll.get(),
            focused,
            app.data_view.sort_column.as_deref(),
            app.data_view.sort_asc,
        )
    } else {
        widgets::panel::render(
            frame,
            area,
            kind,
            &title,
            items,
            panel.selected_idx,
            panel.scroll_offset.get(),
            focused,
            panel.mode,
        )
    };

    // Persistir scroll_offset calculado por el widget
    panel.scroll_offset.set(new_scroll);
}

// ---------------------------------------------------------------------------
// Footer / status bar
// ---------------------------------------------------------------------------

/// Un atajo de la barra inferior: tecla + descripción + estilo semántico.
/// (Patrón `bindingInfo` de lazygit: la barra es DINÁMICA por contexto.)
struct FooterBinding {
    key: &'static str,
    description: &'static str,
    style: ratatui::style::Style,
}

/// Bindings del contexto actual (panel activo + tab Detail + modal abierto).
/// Orden: específicos del contexto primero, globales al final — lazygit
/// muestra los del contexto + globales que no colisionan.
fn footer_bindings(app: &App) -> Vec<FooterBinding> {
    use crate::app::controller::{DetailTab, InputMode};
    use crate::app::panel::PanelKind;
    use crate::ui::theme::THEME;
    use ratatui::style::{Color, Modifier, Style};

    let mut out: Vec<FooterBinding> = Vec::new();
    let normal = Style::default().fg(THEME.dim);
    let accent = Style::default().fg(THEME.selection).add_modifier(Modifier::BOLD);
    let mode = Style::default().fg(Color::Cyan); // modo especial activo
    let danger = Style::default().fg(Color::Red);

    // ── modales abiertos: los atajos del modal dominan ──
    if app.query.query_input.is_some() {
        out.push(FooterBinding { key: "enter", description: "ejecutar", style: accent });
        out.push(FooterBinding { key: "esc", description: "cerrar", style: normal });
        out.push(FooterBinding { key: "↑↓", description: "historial", style: normal });
        out.push(FooterBinding { key: "ctrl+u", description: "limpiar", style: normal });
        out.push(FooterBinding { key: "ctrl+l", description: "limpiar todo", style: normal });
        return out;
    }
    if app.show_row_inspector {
        out.push(FooterBinding { key: "esc", description: "cerrar inspector", style: normal });
        out.push(FooterBinding { key: "shift+j", description: "pares↔json", style: mode });
        return out;
    }
    if app.connection.password_prompt.is_some() {
        out.push(FooterBinding { key: "enter", description: "autenticar", style: accent });
        out.push(FooterBinding { key: "esc", description: "cancelar", style: normal });
        return out;
    }
    if app.connection.db_picker.is_some() {
        out.push(FooterBinding { key: "↑↓", description: "elegir base", style: normal });
        out.push(FooterBinding { key: "enter", description: "conectar", style: accent });
        out.push(FooterBinding { key: "esc", description: "cancelar", style: normal });
        return out;
    }
    if app.input_mode == InputMode::Filtering {
        out.push(FooterBinding { key: "esc", description: "salir filtro", style: normal });
        out.push(FooterBinding { key: "enter", description: "aplicar", style: accent });
        return out;
    }

    // ── sin DB: formulario de conexión ──
    if app.db_path.is_none() {
        out.push(FooterBinding { key: "enter", description: "conectar", style: accent });
        out.push(FooterBinding { key: "tab", description: "siguiente campo", style: normal });
        out.push(FooterBinding { key: "ctrl+u", description: "limpiar campo", style: normal });
        out.push(FooterBinding { key: "ctrl+l", description: "limpiar todo", style: normal });
    }

    // ── panel activo: atajos específicos ──
    match app.active_panel {
        PanelKind::Sources => {
            out.push(FooterBinding { key: "enter", description: "abrir", style: accent });
            out.push(FooterBinding { key: "f", description: "favorito", style: normal });
            out.push(FooterBinding { key: "d", description: "olvidar", style: danger });
        }
        PanelKind::Tables | PanelKind::Views | PanelKind::Advanced => {
            out.push(FooterBinding { key: "enter", description: "ver detalle", style: accent });
            out.push(FooterBinding { key: "/", description: "filtrar", style: normal });
            out.push(FooterBinding { key: "y", description: "copiar", style: normal });
            out.push(FooterBinding { key: "e", description: "exportar csv", style: normal });
            if app.db_path.is_some() && app.active_panel != PanelKind::Advanced {
                out.push(FooterBinding { key: "ctrl+r", description: "contar filas", style: mode });
            }
        }
        PanelKind::Detail => match app.data_view.detail_tab {
            DetailTab::Data => {
                out.push(FooterBinding { key: "j/k", description: "filas", style: normal });
                out.push(FooterBinding { key: "h/l", description: "columnas", style: normal });
                out.push(FooterBinding { key: "o", description: "inspector", style: accent });
                if app.is_nosql {
                    out.push(FooterBinding {
                        key: "shift+j",
                        description: "pares↔json",
                        style: mode,
                    });
                }
            }
            DetailTab::Schema | DetailTab::Sql | DetailTab::Meta => {
                out.push(FooterBinding { key: "j/k", description: "scroll", style: normal });
            }
        },
    }

    // ── globales (siempre visibles, al final) ──
    out.push(FooterBinding { key: "x", description: "menú", style: accent });
    out.push(FooterBinding { key: "?", description: "ayuda", style: normal });
    out.push(FooterBinding { key: "1-5", description: "panel", style: normal });
    out.push(FooterBinding { key: "esc", description: "atrás", style: normal });

    out
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use crate::ui::theme::THEME;
    use ratatui::style::Style;

    // Feedback inmediato: spinner mientras corre una query O una conexión.
    let status = if app.is_loading || app.query.query_state == QueryState::Running {
        let spin = SPINNER[app.frame % SPINNER.len()];
        format!("{spin} {}", app.status)
    } else {
        app.status.clone()
    };

    // Barra DINÁMICA por contexto (patrón lazygit `options_map`): bindings
    // del contexto actual formateados `desc: tecla`, truncando con `…`.
    let bindings = footer_bindings(app);
    let width = usize::from(area.width.saturating_sub(2)); // padding
    let mut line = Vec::<ratatui::text::Span<'_>>::new();
    let mut len = 0usize;
    let sep = " | ";
    let mut truncated = false;

    for (i, b) in bindings.iter().enumerate() {
        let text = format!("{}: {}", b.description, b.key);
        let text_len = text.chars().count();
        let extra = if i > 0 { sep.len() } else { 0 };
        if len + extra + text_len > width {
            truncated = true;
            break;
        }
        if i > 0 {
            line.push(ratatui::text::Span::styled(sep, Style::default().fg(THEME.dim)));
            len += sep.len();
        }
        line.push(ratatui::text::Span::styled(text, b.style));
        len += text_len;
    }
    if truncated {
        line.push(ratatui::text::Span::styled(format!("{sep}…"), Style::default().fg(THEME.dim)));
    }
    // Status al final (spinner + mensaje)
    if len + 2 + status.chars().count() <= width {
        line.push(ratatui::text::Span::styled(
            format!("{sep}{status}"),
            Style::default().fg(THEME.dim),
        ));
    }

    frame.render_widget(ratatui::widgets::Paragraph::new(ratatui::text::Line::from(line)), area);
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

    let border_style = Style::default().fg(crate::ui::theme::THEME.border);
    let block = Block::default()
        .title("Acciones (x/b cerrar, Enter ejecutar)")
        .borders(Borders::ALL)
        .border_style(border_style);

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);
}

// ---------------------------------------------------------------------------
// Password prompt (modal overlay: servidor detectado pide credenciales)
// ---------------------------------------------------------------------------

fn render_password_prompt(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let Some(state) = &app.connection.password_prompt else { return };
    let theme = &crate::ui::theme::THEME;

    let width = area.width.saturating_mul(70) / 100;
    let height = 5;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .title(format!(" Contraseña de {} ", state.user))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.selection));
    let inner = crate::ui::widgets::modal::inner_area(rect);
    frame.render_widget(block, rect);

    // Línea del prompt: usuario + password enmascarado (asteriscos)
    let masked: String = "*".repeat(state.buffer.chars().count());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!("{}@{} ", state.user, state.server_url)),
            Span::styled(masked, Style::default().fg(theme.text)),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Hint de teclas en la última fila del inner
    let hint_y = inner.y + inner.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "  [enter] conectar   [esc] cancelar",
            Style::default().fg(theme.dim),
        )])),
        Rect::new(inner.x, hint_y, inner.width, 1),
    );
}

// ---------------------------------------------------------------------------
// DB picker (modal overlay: SHOW DATABASES de un servidor detectado)
// ---------------------------------------------------------------------------

fn render_db_picker(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

    let Some(state) = &app.connection.db_picker else { return };
    let theme = &crate::ui::theme::THEME;

    let width = area.width.min(52);
    let height = area.height.min(12);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);

    let items: Vec<ListItem<'_>> =
        state.dbs.iter().map(|db| ListItem::new(format!("  {db}"))).collect();
    let mut list_state = ListState::default();
    list_state.select(Some(state.idx));

    let block = Block::default()
        .title(" Elige una base de datos (↑/↓ + Enter, esc cancelar) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().fg(theme.bg).bg(theme.selection))
        .highlight_symbol(">");
    frame.render_stateful_widget(list, rect, &mut list_state);
}
// ---------------------------------------------------------------------------

fn render_help(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    use ratatui::text::Line;

    let sections = app.keymap.help_sections();

    // Ancho de columna de teclas: el mayor de las filas (clamped 10..24)
    let max_keys = sections
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|(k, _)| k.chars().count()))
        .max()
        .unwrap_or(10)
        .clamp(10, 24);

    let mut lines: Vec<Line<'_>> = Vec::new();
    for (title, rows) in &sections {
        if rows.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("── {title} ──"),
            Style::default().fg(crate::ui::theme::THEME.selection).add_modifier(Modifier::BOLD),
        )));
        for (keys, desc) in rows {
            let pad = max_keys.saturating_sub(keys.chars().count());
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {keys}{}", " ".repeat(pad)),
                    Style::default().fg(crate::ui::theme::THEME.unfocused),
                ),
                Span::raw("  "),
                Span::raw(*desc),
            ]));
        }
        lines.push(Line::from(""));
    }

    widgets::modal::render_lines(
        frame,
        area,
        "Ayuda (bindings reales — ?/esc cerrar)",
        &lines,
        &mut app.help_scroll,
        58,
        80,
        None,
    );
}

// ---------------------------------------------------------------------------
// Terminal muy pequeña
// ---------------------------------------------------------------------------

/// Modal de input SQL (`:`): buffer editable con cursor visible, historial
/// navegable debajo (↑/↓, estilo fish) y hint de teclas al pie.
fn render_query_input(frame: &mut Frame<'_>, area: Rect, app: &App) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let Some(state) = &app.query.query_input else { return };
    let theme = &crate::ui::theme::THEME;

    // Tamaño del modal: ancho 70%, alto = borde + input + hasta 8 historial + footer
    let width = area.width.saturating_mul(70) / 100;
    let history_len = app.state.query_history.len().min(8);
    #[allow(clippy::cast_possible_truncation)]
    let height = (4 + history_len).min(12) as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .title(" SQL › ".to_string())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.selection));
    let inner = crate::ui::widgets::modal::inner_area(rect);
    frame.render_widget(block, rect);

    // Fila 0 del inner: prompt + buffer (texto base)
    let prompt = "❯ ";
    let prompt_w = u16::try_from(prompt.chars().count()).unwrap_or(2);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(prompt.to_string()),
            Span::styled(state.buffer.as_str(), Style::default().fg(theme.text)),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Filas 1..N: historial (estilo fish; la entrada activa queda resaltada
    // con inversión fg=bg / bg=selection)
    let footer_y = inner.y + inner.height.saturating_sub(1);
    for (row_y, (i, sql)) in (inner.y + 1..).zip(app.state.query_history.iter().take(8).enumerate())
    {
        if row_y >= footer_y {
            break;
        }
        let selected = state.history_idx == Some(i);
        let style = if selected {
            Style::default().fg(theme.bg).bg(theme.selection)
        } else {
            Style::default().fg(theme.unfocused)
        };
        // Truncar el SQL al ancho útil del inner (dejando 2 de indent + 1 de margen)
        let avail = usize::from(inner.width.saturating_sub(3));
        let text = if sql.chars().count() > avail {
            let truncated: String = sql.chars().take(avail.saturating_sub(1)).collect();
            format!("{truncated}…")
        } else {
            sql.clone()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("  {text}"), style)),
            Rect::new(inner.x, row_y, inner.width, 1),
        );
    }

    // Footer: hint de teclas (patrón lazygit: acciones del modal siempre visibles)
    frame.render_widget(
        Paragraph::new(Span::styled(
            " [enter] ejecutar  [esc] cerrar  [up/down] historial",
            Style::default().fg(theme.unfocused),
        )),
        Rect::new(inner.x, footer_y, inner.width, 1),
    );

    // Cursor real del terminal sobre el buffer (posición del char `cursor`),
    // acotado al tamaño real del frame para que el redimensionado nunca cause
    // panic "index outside of buffer".
    #[allow(clippy::cast_possible_truncation)]
    let cursor_offset = prompt_w + state.cursor as u16;
    let col = (inner.x + cursor_offset).min(frame.area().width.saturating_sub(1));
    let row = inner.y.min(frame.area().height.saturating_sub(1));
    frame.set_cursor_position((col, row));
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect) {
    let msg = "Terminal pequena: amplia ancho/alto para ver lazydb";
    frame.render_widget(Paragraph::new(msg), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// REGRESIÓN: al redimensionar la terminal a una dimensión MENOR, el
    /// layout (computado con el tamaño anterior) sobresale del buffer del
    /// frame → ratatui paniqueaba "index outside of buffer" (scrollbar
    /// horizontal del Data tab y rects de panel sin clamp).
    #[tokio::test]
    async fn render_con_layout_mayor_que_el_frame_no_paniquea() {
        let mut app = App::new();
        // Layout grande (terminal anterior): 120x40
        app.compute_layout(120, 40);
        // Frame REAL pequeño (terminal nueva): 30x10 → el layout sobresale
        let backend = TestBackend::new(30, 10);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal de test");
        terminal
            .draw(|frame| render(frame, &mut app))
            .expect("render con resize no debe paniquear");
    }

    /// REGRESIÓN (escenario real del usuario): resize con Data tab poblado
    /// y muchas columnas — el thumb del scrollbar horizontal se dibujaba
    /// con `.expect()` y paniqueaba al sobresalir del buffer nuevo.
    #[tokio::test]
    async fn render_resize_con_scrollbar_horizontal_no_paniquea() {
        let mut app = App::new();
        // Data tab con MUCHAS columnas → h_scroll activo (thumb dibujado).
        // El resize a un frame pequeño hace que el rect del panel sobresalga.
        app.data_view.detail_tab = DetailTab::Data;
        app.tables = vec!["t".to_string()];
        let headers: Vec<String> = (0..30).map(|i| format!("col_{i}")).collect();
        let mut rows = vec![headers.join(" | ")];
        rows.extend(
            (0..50).map(|i| (0..30).map(|c| format!("v{i}_{c}")).collect::<Vec<_>>().join(" | ")),
        );
        app.data_view.preview_rows = rows;

        // Layout grande → frame pequeño (resize race)
        app.compute_layout(150, 40);
        let backend = ratatui::backend::TestBackend::new(25, 8);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame, &mut app))
            .expect("render con h_scroll + resize no debe paniquear");
    }
}

#[cfg(test)]
mod footer_tests {
    use super::*;
    use crate::app::controller::App;
    use crate::app::panel::PanelKind;

    fn descs(app: &App) -> Vec<String> {
        footer_bindings(app).iter().map(|b| b.description.to_string()).collect()
    }

    #[test]
    fn sin_db_muestra_bindings_del_formulario() {
        let app = App::new();
        let d = descs(&app);
        assert!(d.iter().any(|x| x == "conectar"), "formulario: {d:?}");
        assert!(d.iter().any(|x| x == "siguiente campo"), "formulario: {d:?}");
    }

    #[test]
    fn sources_muestra_favorito_y_olvidar() {
        let app = App::new();
        let d = descs(&app);
        assert!(d.iter().any(|x| x == "favorito"), "sources: {d:?}");
        assert!(d.iter().any(|x| x == "olvidar"), "sources: {d:?}");
        assert!(d.iter().any(|x| x == "abrir"), "sources: {d:?}");
    }

    #[test]
    fn data_tab_muestra_inspector() {
        let mut app = App::new();
        app.data_view.detail_tab = crate::app::controller::DetailTab::Data;
        app.active_panel = PanelKind::Detail;
        let d = descs(&app);
        assert!(d.iter().any(|x| x == "inspector"), "data: {d:?}");
        assert!(d.iter().any(|x| x == "columnas"), "data: {d:?}");
    }

    #[test]
    fn modal_query_domina_los_bindings() {
        let mut app = App::new();
        app.query.query_input = Some(crate::query::QueryInputState::default());
        let d = descs(&app);
        assert!(d.iter().any(|x| x == "ejecutar"), "modal: {d:?}");
        // El modal no muestra bindings de panel
        assert!(!d.iter().any(|x| x == "favorito"), "modal: {d:?}");
    }

    #[test]
    fn globales_siempre_presentes() {
        let app = App::new();
        let d = descs(&app);
        assert!(d.iter().any(|x| x == "menú"), "global: {d:?}");
        assert!(d.iter().any(|x| x == "ayuda"), "global: {d:?}");
    }
}
