//! musiq — lecteur de musique en mode terminal : musique du PC et d'un serveur Jellyfin.

mod app;
mod config;
mod disc;
mod jellyfin;
mod local;
mod model;
mod player;
mod queue;
mod ui;

use app::{App, Msg};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn main() -> io::Result<()> {
    let cfg = config::Config::load();
    let (tx, rx) = mpsc::channel();
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let mut app = App::new(cfg, tx);
    let res = run(&mut terminal, &mut app, &rx);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    app.shutdown();
    res
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App, rx: &Receiver<Msg>) -> io::Result<()> {
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            // On vide tout ce qui est en attente avant de redessiner.
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => {}
                }
                if app.quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        while let Ok(m) = rx.try_recv() {
            app.on_msg(m);
        }
        app.tick();
    }
    Ok(())
}
