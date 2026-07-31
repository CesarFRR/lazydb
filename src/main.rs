mod app;
mod config;
mod db;
mod keys;
mod paths;
mod query;
mod storage;
mod ui;

use std::{io, time::Duration};

use app::App;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;

#[tokio::main]
async fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
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
