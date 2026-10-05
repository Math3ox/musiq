//! musiq — lecteur de musique en mode terminal : musique du PC et d'un serveur Jellyfin.

mod app;
mod config;
mod disc;
mod eq;
mod jellyfin;
mod local;
mod model;
mod player;
mod queue;
mod ui;

use app::{App, Msg};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;

fn main() -> io::Result<()> {
    let cfg = config::Config::load();
    let (tx, rx) = mpsc::channel();
    // Clavier et souris arrivent par le même canal que mpv et le réseau :
    // la boucle principale peut dormir tant que rien ne se passe.
    let input = tx.clone();
    thread::spawn(move || {
        while let Ok(e) = event::read() {
            if input.send(Msg::Input(e)).is_err() {
                break;
            }
        }
    });
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
    let mut dirty = true;
    while !app.quit {
        if dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        // Attente jusqu'au prochain message, ou à la prochaine échéance (image du
        // CD, effacement d'un message). En pause, sans échéance : on dort.
        let first = match app.deadline() {
            Some(d) => match rx.recv_timeout(d) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        if let Some(m) = first {
            dirty |= app.on_msg(m);
            while let Ok(m) = rx.try_recv() {
                dirty |= app.on_msg(m);
            }
        }
        dirty |= app.tick();
    }
    Ok(())
}
