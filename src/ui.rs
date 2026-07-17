use chrono::{DateTime, Local};
use nostr_sdk::prelude::*;
use ratatui::style::Stylize;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use ratatui_image::{FilterType, Resize, StatefulImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, CustomEmoji, InputMode, QuoteDisplay, RenderedPart, ReplyDisplay, TimelineTab,
};

const ACCENT: Color = Color::Rgb(180, 140, 255);
const DIM: Color = Color::Rgb(130, 135, 150);
const AVATAR_WIDTH: u16 = 4;
const AVATAR_HEIGHT: u16 = 2;
const AVATAR_INDENT: &str = "      ";

pub fn draw(frame: &mut Frame, app: &mut App) {
    let input_height = if matches!(app.mode, InputMode::Normal) {
        1
    } else {
        6
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, rows[0]);
    if app.detail {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(rows[1]);
        draw_timeline(frame, app, columns[0]);
        draw_detail(frame, app, columns[1]);
    } else {
        draw_timeline(frame, app, rows[1]);
    }
    draw_input(frame, app, rows[2]);
    draw_footer(frame, app, rows[3]);
    if app.settings_open() {
        draw_settings(frame, app);
    }
}

mod presentation;
mod timeline;

#[cfg(test)]
use presentation::content_span;
use presentation::{
    compact_content_line, detailed_content_layout, quote_lines, render_avatar,
    render_custom_emojis, reply_line, repost_line,
};
#[cfg(test)]
use timeline::timeline_render_window;
use timeline::{draw_detail, draw_timeline};
mod chrome;
mod editor;

use chrome::{compact, draw_footer, draw_header, draw_settings, format_time};

use editor::draw_input;
#[cfg(test)]
use editor::{editor_layout, EditorLayout};

#[cfg(test)]
mod tests;
