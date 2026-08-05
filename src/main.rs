mod app;
mod config;
mod db;
mod keys;
mod paths;
mod query;
mod security;
mod storage;
mod ui;

use std::{fs, io, path::PathBuf, time::Duration};

use app::App;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
        EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
};
use tracing_appender::non_blocking::WorkerGuard;

/// Inicializa el logger: archivo rotativo diario en
/// `~/.config/lazydb/logs/lazydb.YYYY-MM-DD.log` (patrón lazygit: la app
/// NUNCA imprime en stdout, todo va a disco; el usuario ve solo la TUI).
fn init_tracing() -> io::Result<WorkerGuard> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let log_dir = PathBuf::from(home).join(".config").join("lazydb").join("logs");
    fs::create_dir_all(&log_dir)?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, "lazydb.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .with_level(true)
        .init();
    tracing::info!(dir = %log_dir.display(), "logs inicializados");
    Ok(guard)
}

/// Hook de pánico: log a disco + restaurar la terminal ANTES de que el
/// proceso muera (sin esto, un panic dejaba la terminal en raw mode /
/// alternate screen y había que reiniciarla a mano). Encadenamos al hook
/// interno de `ratatui::init()` para preservar su restore completo
/// (raw mode + alternate screen + mouse capture + cursor).
fn install_panic_hook() {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 1) Logging a disco (nuestra parte)
        tracing::error!(panic = %info, "panic: terminal restaurada");
        // 2) Desactivar la captura del ratón explícitamente: si el panic
        // ocurrió con mouse capture activo, el hook interno de ratatui no
        // siempre lo desactiva. Lo hacemos antes para no dejar la terminal
        // enviando eventos crudos al proceso zombie.
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        // 3) Restaurar la terminal (delegado al hook interno de ratatui).
        prev_hook(info);
        // 4) Mensaje al stderr para que el usuario sepa qué pasó (después
        // del restore, ya en modo cooked).
        eprintln!("\n[panic] lazydb: {info}\nDetalles en ~/.config/lazydb/logs/");
    }));
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let _log_guard = init_tracing()?;
    tracing::debug!("arrancando lazydb");

    // Setup estándar de ratatui (raw mode + alternate screen) con panic
    // hook integrado; añadimos el nuestro para loggear a disco.
    let mut terminal = ratatui::init();
    install_panic_hook();
    execute!(terminal.backend_mut(), EnableMouseCapture)?;
    // Bracketed paste: pegar texto multilínea (p.ej. URLs partidas por
    // CleverCloud) llega como UN Event::Paste en vez de chars sueltos
    // donde el `\n` se interpretaría como Enter.
    execute!(terminal.backend_mut(), EnableBracketedPaste)?;

    let mut app = App::new();
    let result = run_app(&mut terminal, &mut app);

    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    ratatui::restore();
    tracing::debug!(salida = ?result.is_ok(), "cerrando lazydb");

    result
}

fn run_app(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        // Calcular layout antes de renderizar
        let size = terminal.size()?;
        app.compute_layout(size.width, size.height);

        terminal.draw(|frame| ui::render(frame, app))?;

        if app.should_quit {
            return Ok(());
        }

        // Esperar eventos con timeout corto (respuesta fluida del mouse).
        // Una vez que llega el primero, se procesan TODOS los pendientes
        // (drain) antes de redibujar, para que el drag no se sienta pesado.
        if event::poll(Duration::from_millis(50))? {
            loop {
                match event::read()? {
                    // Bracketed paste: texto pegado (puede ser multilínea,
                    // p.ej. URLs de CleverCloud partidas). Va directo al
                    // estado activo (formulario de conexión, input, etc.).
                    Event::Paste(text) => {
                        app.on_paste(&text);
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Ctrl+C = cierre seguro: primero cierra filtro/menús/
                        // modales abiertos, y solo sale si no queda nada abierto
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c')
                        {
                            app.on_ctrl_c();
                        } else {
                            app.on_key(key);
                        }
                    }
                    Event::Mouse(mouse)
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
                    {
                        let size = terminal.size()?;
                        // Decide si es click en barra de scroll (drag) o click normal
                        app.on_mouse_down(mouse.column, mouse.row, size.width, size.height);
                    }
                    Event::Mouse(mouse)
                        if mouse.kind == MouseEventKind::Drag(MouseButton::Left) =>
                    {
                        // Arrastre de barra de scroll (click + mover)
                        app.on_mouse_drag(mouse.column, mouse.row);
                    }
                    Event::Mouse(mouse) if mouse.kind == MouseEventKind::Up(MouseButton::Left) => {
                        // Soltar botón → terminar arrastre
                        app.on_mouse_up();
                    }
                    Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollUp => {
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            // shift+rueda arriba → columnas a la izquierda
                            app.on_h_scroll(-1);
                        } else {
                            app.on_scroll(true, mouse.column, mouse.row);
                        }
                    }
                    Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollDown => {
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            // shift+rueda abajo → columnas a la derecha
                            app.on_h_scroll(1);
                        } else {
                            app.on_scroll(false, mouse.column, mouse.row);
                        }
                    }
                    // shift+rueda (terminales que emiten ScrollLeft/ScrollRight)
                    Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollLeft => {
                        app.on_h_scroll(-1);
                    }
                    Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollRight => {
                        app.on_h_scroll(1);
                    }
                    _ => {}
                }
                // Salir del drain cuando no queden eventos encolados
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}
