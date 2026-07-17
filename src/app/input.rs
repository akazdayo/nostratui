use super::*;

impl App {
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if self.settings_open {
            return match key.code {
                KeyCode::Char('m') | KeyCode::Esc => {
                    self.settings_open = false;
                    None
                }
                KeyCode::Char('q') => Some(Command::Quit),
                _ => None,
            };
        }
        match self.mode.clone() {
            InputMode::Normal => self.on_normal_key(key),
            InputMode::Compose { reply_to } => self.on_input_key(key, reply_to, None),
            InputMode::Reaction { event } => self.on_input_key(key, None, Some(event)),
        }
    }

    fn on_normal_key(&mut self, key: KeyEvent) -> Option<Command> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => Some(Command::Quit),
            (KeyCode::Tab, _) | (KeyCode::Char(']'), _) => {
                self.select_tab(match self.active_tab {
                    TimelineTab::Following => TimelineTab::Global,
                    TimelineTab::Global => TimelineTab::Following,
                });
                None
            }
            (KeyCode::BackTab, _) | (KeyCode::Char('['), _) => {
                self.select_tab(match self.active_tab {
                    TimelineTab::Following => TimelineTab::Global,
                    TimelineTab::Global => TimelineTab::Following,
                });
                None
            }
            (KeyCode::Char('1'), _) => {
                self.select_tab(TimelineTab::Following);
                None
            }
            (KeyCode::Char('2'), _) => {
                self.select_tab(TimelineTab::Global);
                None
            }
            (KeyCode::Char('m'), _) => {
                self.settings_open = true;
                None
            }
            (KeyCode::Char('j') | KeyCode::Down, _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected =
                    (timeline.selected + 1).min(timeline.events.len().saturating_sub(1));
                None
            }
            (KeyCode::Char('k') | KeyCode::Up, _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected = timeline.selected.saturating_sub(1);
                None
            }
            (KeyCode::Char('g'), _) => {
                let timeline = self.timeline_state_mut();
                timeline.selected = 0;
                resume_timeline(timeline);
                None
            }
            (KeyCode::Char('G'), _) => {
                let timeline = self.timeline_state_mut();
                if !timeline.events.is_empty() {
                    timeline.live = false;
                }
                timeline.selected = timeline.events.len().saturating_sub(1);
                None
            }
            (KeyCode::Char('l') | KeyCode::Enter, _) => {
                self.detail = true;
                None
            }
            (KeyCode::Char('h') | KeyCode::Esc, _) => {
                self.detail = false;
                None
            }
            (KeyCode::Char('i' | 'o'), _) => {
                self.begin_compose(None);
                None
            }
            (KeyCode::Char('r'), _) => {
                if let Some(event) = self.selected_content_event() {
                    self.begin_compose(Some(Box::new(event)));
                }
                None
            }
            (KeyCode::Char('+'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                self.selected_content_event().map(|event| Command::React {
                    event: Box::new(event),
                    reaction: "+".to_owned(),
                })
            }
            (KeyCode::Char('-'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                self.selected_content_event().map(|event| Command::React {
                    event: Box::new(event),
                    reaction: "-".to_owned(),
                })
            }
            (KeyCode::Char('e'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                if let Some(event) = self.selected_content_event() {
                    self.mode = InputMode::Reaction {
                        event: Box::new(event),
                    };
                    self.input.clear();
                }
                None
            }
            (KeyCode::Char('R'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to repost".to_owned();
                    None
                } else {
                    self.selected_content_event()
                        .map(|event| Command::Repost(Box::new(event)))
                }
            }
            _ => None,
        }
    }

    fn on_input_key(
        &mut self,
        key: KeyEvent,
        reply_to: Option<Box<Event>>,
        reaction_to: Option<Box<Event>>,
    ) -> Option<Command> {
        if key.code == KeyCode::Esc {
            self.mode = InputMode::Normal;
            self.input.clear();
            self.status = "cancelled".to_owned();
            return None;
        }
        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let content = self.input.trim().to_owned();
            if content.is_empty() {
                self.status = "nothing to send".to_owned();
                return None;
            }
            self.mode = InputMode::Normal;
            self.input.clear();
            self.status = "sending…".to_owned();
            return if let Some(event) = reaction_to {
                Some(Command::React {
                    event,
                    reaction: content,
                })
            } else {
                Some(Command::Publish { content, reply_to })
            };
        }

        match key.code {
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character)
            }
            KeyCode::Enter if reaction_to.is_none() => self.input.push('\n'),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Tab => self.input.push('\t'),
            _ => {}
        }
        None
    }

    fn begin_compose(&mut self, reply_to: Option<Box<Event>>) {
        if self.read_only {
            self.status = "read-only: set NOSTR_SECRET_KEY to publish".to_owned();
            return;
        }
        self.mode = InputMode::Compose { reply_to };
        self.input.clear();
    }
}
