//! Formulario "Nueva conexión" del panel Detail (sin db abierta).
//!
//! Renderiza el formulario con detección en vivo del tipo de base:
//! - Escribir una URL/ruta completa → se rellenan los campos (debounce 1s)
//! - Editar un campo → la URL se reconcatena al instante
//! - Tipos archivo (sqlite/duckdb/csv...) → campos remotos deshabilitados
//!
//! Las teclas se manejan en el controller (`handle_connection_form_key`).
//! Este módulo SOLO pinta el estado actual.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{controller::{App, ConnField}, PanelKind};
use crate::db::connection::ConnectionType;

/// Ancho del label de cada campo (alinea los inputs en columna).
const LABEL_W: u16 = 16;
/// Ancho del input (caja de texto de cada campo).
const INPUT_W: u16 = 38;

/// Renderiza el formulario de conexión dentro del área del panel Detail.
// El `y += 1` tras cada rama es intencional (layout por filas).
#[allow(clippy::branches_sharing_code)]
pub fn render(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(form) = &app.connection_form else { return };
    if area.width < 60 || area.height < 18 {
        return;
    }

    let focused = app.active_panel == PanelKind::Detail;
    let block = Block::default()
        .title(" Nueva conexión ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { crate::ui::theme::THEME.selection } else { crate::ui::theme::THEME.border }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut y = inner.y;
    let x = inner.x;

    // ── campo URL (maestro) ──
    y = field(frame, x, y, "URL o ruta", &form.url, form.active == ConnField::Url, INPUT_W);
    y += 1;
    // nota de detección
    let note_style = if form.detected_note.starts_with("✓") {
        Style::default().fg(Color::Green)
    } else if form.detected_note.starts_with('⚠') {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(crate::ui::theme::THEME.dim)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(&form.detected_note, note_style)))
            .wrap(Wrap { trim: false }),
        Rect::new(x + LABEL_W, y, INPUT_W, 1),
    );
    y += 1;

    // ── separador ──
    let sep = "─".repeat((INPUT_W + LABEL_W) as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(sep, Style::default().fg(crate::ui::theme::THEME.dim)))),
        Rect::new(x, y, INPUT_W + LABEL_W, 1),
    );
    y += 1;

    // ── Tipo (auto-detectado o forzado) ──
    let kind_label = form.kind_override.unwrap_or(form.kind).label();
    y = field(frame, x, y, "Tipo", kind_label, form.active == ConnField::Kind, INPUT_W);
    y += 1;

    let is_file = matches!(
        form.kind_override.unwrap_or(form.kind),
        ConnectionType::Sqlite | ConnectionType::Duckdb | ConnectionType::File
    );

    if is_file {
        // Tipo archivo: campos remotos atenuados (deshabilitados)
        y = disabled_field(frame, x, y, "Host", "— (archivo local)");
        y += 1;
        y = disabled_field(frame, x, y, "Puerto", "—");
        y += 1;
        y = disabled_field(frame, x, y, "Usuario", "—");
        y += 1;
        y = disabled_field(frame, x, y, "Contraseña", "—");
        y += 1;
        y = disabled_field(frame, x, y, "Base de datos", "—");
        y += 1;
    } else {
        // Remoto (mysql/postgres/mongo) o desconocido: campos editables
        y = field(frame, x, y, "Host", &form.host, form.active == ConnField::Host, INPUT_W);
        y += 1;
        y = field(frame, x, y, "Puerto", &form.port, form.active == ConnField::Port, INPUT_W);
        y += 1;
        y = field(frame, x, y, "Usuario", &form.user, form.active == ConnField::User, INPUT_W);
        y += 1;
        y = field(frame, x, y, "Contraseña", &mask(&form.pass), form.active == ConnField::Pass, INPUT_W);
        y += 1;
        y = field(frame, x, y, "Base de datos", &form.db, form.active == ConnField::Db, INPUT_W);
        y += 1;
    }

    y += 1;

    // ── botón Conectar + spinner ──
    if form.connecting {
        let sp = "▓▓▓▓▓▓░░░░░░";
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Conectando… ", Style::default().fg(Color::Yellow)),
                Span::styled(sp, Style::default().fg(Color::Cyan)),
            ])),
            Rect::new(x, y, INPUT_W + LABEL_W, 1),
        );
        y += 2;
    } else {
        let connect_style = if form.active == ConnField::Connect {
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" [Conectar] ", connect_style),
                Span::styled(" [Examinar…] ", Style::default().fg(crate::ui::theme::THEME.dim)),
                Span::styled(" (?) ", Style::default().fg(crate::ui::theme::THEME.dim)),
            ])),
            Rect::new(x, y, INPUT_W + LABEL_W, 1),
        );
        y += 1;
    }

    // ── footer de teclas ──
    y += 1;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Tab: siguiente campo · ↑/↓: navegar · Enter: conectar · Esc: salir",
            Style::default().fg(crate::ui::theme::THEME.dim),
        ))),
        Rect::new(x, y, INPUT_W + LABEL_W, 1),
    );
}

/// Renderiza un campo label + input. Devuelve la Y siguiente.
fn field(
    frame: &mut Frame<'_>,
    x: u16,
    y: u16,
    label: &str,
    value: &str,
    active: bool,
    input_w: u16,
) -> u16 {
    let label_style = if active {
        Style::default().fg(crate::ui::theme::THEME.selection).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(crate::ui::theme::THEME.dim)
    };
    let input_style = if active {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("{label:<label_w$}", label_w = LABEL_W as usize), label_style))),
        Rect::new(x, y, LABEL_W, 1),
    );
    let trimmed: String = value.chars().take(input_w as usize).collect();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("[{trimmed:<w$}]", w = input_w as usize), input_style))),
        Rect::new(x + LABEL_W, y, input_w, 1),
    );
    y + 1
}

/// Campo deshabilitado (tipo archivo): atenuado y sin caja editable.
fn disabled_field(frame: &mut Frame<'_>, x: u16, y: u16, label: &str, value: &str) -> u16 {
    let style = Style::default().fg(crate::ui::theme::THEME.dim);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("{label:<label_w$}", label_w = LABEL_W as usize), style))),
        Rect::new(x, y, LABEL_W, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {value}"), style))),
        Rect::new(x + LABEL_W, y, INPUT_W, 1),
    );
    y + 1
}

/// Enmascara la contraseña con `•`.
fn mask(pass: &str) -> String {
    if pass.is_empty() {
        String::new()
    } else {
        "•".repeat(pass.chars().count())
    }
}
