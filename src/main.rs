mod app;
mod graphics;
mod network;
mod ui;

use std::{
    io, panic,
    time::{Duration, Instant},
};

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
const SCROLL_EVENT_DEBOUNCE: Duration = Duration::from_millis(25);

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
    let mut input_state = TerminalInputState::default();
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

            let batch = apply_terminal_events(app, events, &mut input_state, Instant::now());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollDirection {
    Down,
    Up,
}

fn scroll_direction(terminal_event: &TerminalEvent) -> Option<ScrollDirection> {
    let TerminalEvent::Key(key) = terminal_event else {
        return None;
    };
    if key.kind == crossterm::event::KeyEventKind::Release {
        return None;
    }
    match key.code {
        crossterm::event::KeyCode::Char('j') | crossterm::event::KeyCode::Down => {
            Some(ScrollDirection::Down)
        }
        crossterm::event::KeyCode::Char('k') | crossterm::event::KeyCode::Up => {
            Some(ScrollDirection::Up)
        }
        _ => None,
    }
}

#[derive(Default)]
struct TerminalInputState {
    last_scroll: Option<(ScrollDirection, Instant)>,
}

impl TerminalInputState {
    fn accepts(&mut self, terminal_event: &TerminalEvent, now: Instant) -> bool {
        let Some(direction) = scroll_direction(terminal_event) else {
            return true;
        };
        if self.last_scroll.is_some_and(|(last_direction, last_at)| {
            last_direction == direction
                && now.saturating_duration_since(last_at) < SCROLL_EVENT_DEBOUNCE
        }) {
            return false;
        }
        self.last_scroll = Some((direction, now));
        true
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
    input_state: &mut TerminalInputState,
    now: Instant,
) -> TerminalEventBatch {
    let mut batch = TerminalEventBatch::default();
    for terminal_event in events {
        if !input_state.accepts(&terminal_event, now) {
            continue;
        }
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
    fn duplicate_scroll_events_apply_only_once() {
        let keys = Keys::generate();
        let mut app = App::new(true, Vec::new());
        for timestamp in 1..=100 {
            let event = EventBuilder::text_note(format!("note {timestamp}"))
                .custom_created_at(Timestamp::from_secs(timestamp))
                .sign_with_keys(&keys)
                .unwrap();
            app.on_ui_event(UiEvent::Event(Box::new(event)));
        }
        let scroll = || TerminalEvent::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        let now = Instant::now();
        let mut input_state = TerminalInputState::default();

        let first_frame = apply_terminal_events(
            &mut app,
            [scroll(), scroll(), scroll()],
            &mut input_state,
            now,
        );
        assert_eq!(app.selected_index(), 1);
        let duplicate_frame = apply_terminal_events(
            &mut app,
            [scroll()],
            &mut input_state,
            now + SCROLL_EVENT_DEBOUNCE / 2,
        );
        assert_eq!(app.selected_index(), 1);
        let repeat_frame = apply_terminal_events(
            &mut app,
            [scroll()],
            &mut input_state,
            now + SCROLL_EVENT_DEBOUNCE,
        );

        assert_eq!(app.selected_index(), 2);
        assert!(first_frame.needs_redraw);
        assert!(!duplicate_frame.needs_redraw);
        assert!(repeat_frame.needs_redraw);
    }
}
