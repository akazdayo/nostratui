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
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use network::{NetworkConfig, UiEvent};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

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
    loop {
        while let Ok(message) = ui_rx.try_recv() {
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

        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let TerminalEvent::Key(key) = event::read()? {
                if let Some(command) = app.on_key(key) {
                    if matches!(command, Command::Quit) {
                        return Ok(());
                    }
                    command_tx
                        .send(command)
                        .await
                        .context("network task stopped")?;
                }
            }
        }
    }
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
