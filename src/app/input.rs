use super::*;
use unicode_segmentation::UnicodeSegmentation;

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
            (KeyCode::Char('f'), _) => {
                if self.read_only {
                    self.status = "read-only: set NOSTR_SECRET_KEY to react".to_owned();
                    return None;
                }
                self.selected_content_event().map(|event| Command::React {
                    event: Box::new(event),
                    reaction: "+".to_owned(),
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
                    self.input_cursor = 0;
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
            self.input_cursor = 0;
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
            self.input_cursor = 0;
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
                self.input.insert(self.input_cursor, character);
                self.input_cursor += character.len_utf8();
            }
            KeyCode::Enter if reaction_to.is_none() => {
                self.input.insert(self.input_cursor, '\n');
                self.input_cursor += '\n'.len_utf8();
            }
            KeyCode::Backspace => {
                let previous = previous_grapheme_boundary(&self.input, self.input_cursor);
                self.input.replace_range(previous..self.input_cursor, "");
                self.input_cursor = previous;
            }
            KeyCode::Delete => {
                let next = next_grapheme_boundary(&self.input, self.input_cursor);
                self.input.replace_range(self.input_cursor..next, "");
            }
            KeyCode::Left => {
                self.input_cursor = previous_grapheme_boundary(&self.input, self.input_cursor);
            }
            KeyCode::Right => {
                self.input_cursor = next_grapheme_boundary(&self.input, self.input_cursor);
            }
            KeyCode::Home => {
                self.input_cursor = line_start(&self.input, self.input_cursor);
            }
            KeyCode::End => {
                self.input_cursor = line_end(&self.input, self.input_cursor);
            }
            KeyCode::Up => {
                self.input_cursor = move_to_adjacent_line(&self.input, self.input_cursor, false);
            }
            KeyCode::Down => {
                self.input_cursor = move_to_adjacent_line(&self.input, self.input_cursor, true);
            }
            KeyCode::Tab => {
                self.input.insert(self.input_cursor, '\t');
                self.input_cursor += '\t'.len_utf8();
            }
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
        self.input_cursor = 0;
    }
}

fn previous_grapheme_boundary(input: &str, cursor: usize) -> usize {
    input[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_grapheme_boundary(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .graphemes(true)
        .next()
        .map_or(cursor, |grapheme| cursor + grapheme.len())
}

fn line_start(input: &str, cursor: usize) -> usize {
    input[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .find('\n')
        .map_or(input.len(), |index| cursor + index)
}

fn move_to_adjacent_line(input: &str, cursor: usize, down: bool) -> usize {
    let current_start = line_start(input, cursor);
    let column = input[current_start..cursor].graphemes(true).count();
    let (target_start, target_end) = if down {
        let current_end = line_end(input, cursor);
        if current_end == input.len() {
            return cursor;
        }
        let target_start = current_end + 1;
        (target_start, line_end(input, target_start))
    } else {
        if current_start == 0 {
            return cursor;
        }
        let target_end = current_start - 1;
        (line_start(input, target_end), target_end)
    };

    input[target_start..target_end]
        .grapheme_indices(true)
        .nth(column)
        .map_or(target_end, |(index, _)| target_start + index)
}
