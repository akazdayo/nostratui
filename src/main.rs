mod app;
mod graphics;
mod network;
mod ui;

use std::{io, panic, time::Duration};

use anyhow::{Context, Result};
use app::{App, Command};
use clap::Parser;
use crossterm::{
    event::{self, Event as TerminalEvent},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use network::{NetworkConfig, UiEvent};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

const MAX_TERMINAL_EVENTS_PER_FRAME: usize = 64;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// WebSocket relay URL. Repeat the option or separate values with commas.
    #[arg(
        short,
        long = "relay",
        value_delimiter = ',',
        default_value = "wss://yabu.me"
    )]
    relays: Vec<String>,

    /// nsec or hexadecimal secret key (prefer NOSTR_SECRET_KEY).
    #[arg(long, env = "NOSTR_SECRET_KEY", hide_env_values = true)]
    secret_key: Option<String>,

    /// Number of recent events requested from relays.
    #[arg(long, default_value_t = 200)]
    limit: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    install_panic_hook();

    let (command_tx, command_rx) = mpsc::channel(64);
    let (ui_tx, mut ui_rx) = mpsc::channel(512);
    let config = NetworkConfig {
        relays: args.relays,
        secret_key: args.secret_key,
        limit: args.limit,
    };
    let read_only = config.secret_key.is_none();
    let relays = config.relays.clone();
    let network_task = tokio::spawn(network::run(config, command_rx, ui_tx));

    let mut terminal = setup_terminal()?;
    let mut app = App::new(read_only, relays);
    app.set_image_cache(graphics::ImageCache::detect().unwrap_or_default());
    let result = run_app(&mut terminal, &mut app, command_tx, &mut ui_rx).await;

    app.clear_images();
    let image_cleanup = flush_deleted_images(&mut terminal, &mut app);
    let terminal_cleanup = restore_terminal(&mut terminal);
    network_task.abort();
    result?;
    image_cleanup?;
    terminal_cleanup
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    command_tx: mpsc::Sender<Command>,
    ui_rx: &mut mpsc::Receiver<UiEvent>,
) -> Result<()> {
    let mut needs_redraw = true;
    loop {
        while let Ok(message) = ui_rx.try_recv() {
            needs_redraw = true;
            if let Some(command) = app.on_ui_event(message) {
                command_tx
                    .send(command)
                    .await
                    .context("network task stopped")?;
            }
        }

        for command in app
            .reference_commands()
            .into_iter()
            .chain(app.image_commands())
        {
            command_tx
                .send(command)
                .await
                .context("network task stopped")?;
        }

        flush_deleted_images(terminal, app)?;

        if needs_redraw {
            draw_frame(terminal, app)?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(50))? {
            let mut events = Vec::with_capacity(MAX_TERMINAL_EVENTS_PER_FRAME);
            events.push(event::read()?);
            while events.len() < MAX_TERMINAL_EVENTS_PER_FRAME && event::poll(Duration::ZERO)? {
                events.push(event::read()?);
            }

            let batch = apply_terminal_events(app, events);
            if batch.quit {
                return Ok(());
            }
            needs_redraw |= batch.needs_redraw;
            for command in batch.commands {
                command_tx
                    .send(command)
                    .await
                    .context("network task stopped")?;
            }
        }
    }
}

#[derive(Default)]
struct TerminalEventBatch {
    needs_redraw: bool,
    quit: bool,
    commands: Vec<Command>,
}

fn apply_terminal_events(
    app: &mut App,
    events: impl IntoIterator<Item = TerminalEvent>,
) -> TerminalEventBatch {
    let mut batch = TerminalEventBatch::default();
    for terminal_event in events {
        match terminal_event {
            TerminalEvent::Key(key) => {
                batch.needs_redraw = true;
                if let Some(command) = app.on_key(key) {
                    if matches!(command, Command::Quit) {
                        batch.quit = true;
                        break;
                    }
                    batch.commands.push(command);
                }
            }
            TerminalEvent::Resize(_, _) => batch.needs_redraw = true,
            _ => {}
        }
    }
    batch
}

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let draw_result = terminal.draw(|frame| ui::draw(frame, app)).map(|_| ());
    // Always release synchronized mode, including when drawing fails.
    let end_result = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    draw_result.and(end_result)
}

fn flush_deleted_images(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    let ids = app.take_deleted_image_ids();
    graphics::delete_kitty_images(terminal.backend_mut(), &ids)
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = graphics::delete_all_kitty_images(&mut stdout);
        let _ = execute!(stdout, LeaveAlternateScreen);
        original(info);
    }));
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nostr_sdk::prelude::*;

    use super::*;

    #[test]
    fn rapid_scroll_events_are_applied_as_one_redraw_batch() {
        let keys = Keys::generate();
        let mut app = App::new(true, Vec::new());
        for timestamp in 1..=100 {
            let event = EventBuilder::text_note(format!("note {timestamp}"))
                .custom_created_at(Timestamp::from_secs(timestamp))
                .sign_with_keys(&keys)
                .unwrap();
            app.on_ui_event(UiEvent::Event(Box::new(event)));
        }
        let events = (0..40)
            .map(|_| TerminalEvent::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));

        let batch = apply_terminal_events(&mut app, events);

        assert_eq!(app.selected_index(), 40);
        assert!(batch.needs_redraw);
        assert!(!batch.quit);
        assert!(batch.commands.is_empty());
    }
}
