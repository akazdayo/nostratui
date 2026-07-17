use super::*;

pub(super) fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        InputMode::Normal => {}
        InputMode::Compose { reply_to } => {
            let title = if reply_to.is_some() {
                " Reply · Ctrl-S send · Esc cancel "
            } else {
                " New note · Ctrl-S send · Esc cancel "
            };
            draw_editor(frame, app, area, title);
        }
        InputMode::Reaction { .. } => {
            draw_editor(
                frame,
                app,
                area,
                " Emoji reaction · Ctrl-S send · Esc cancel ",
            );
        }
    }
}

fn draw_editor(frame: &mut Frame, app: &App, area: Rect, title: &str) {
    let inner_width = area.width.saturating_sub(2).max(1);
    let inner_height = area.height.saturating_sub(2).max(1);
    let layout = editor_layout(&app.input, app.input_cursor(), inner_width);
    let scroll = layout
        .cursor_row
        .saturating_sub(usize::from(inner_height.saturating_sub(1)));
    let text = Text::from(
        layout
            .lines
            .iter()
            .map(|line| Line::raw(line.as_str()))
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().title(title).borders(Borders::ALL))
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        area,
    );
    let x = area.x + 1 + layout.cursor_column.min(usize::from(inner_width - 1)) as u16;
    let visible_row = layout.cursor_row.saturating_sub(scroll);
    let y = area.y + 1 + visible_row.min(usize::from(inner_height - 1)) as u16;
    frame.set_cursor_position((x, y));
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EditorLayout {
    pub(super) lines: Vec<String>,
    pub(super) cursor_column: usize,
    pub(super) cursor_row: usize,
}

/// Hard-wraps editor input using the same terminal-cell widths used by ratatui.
/// Grapheme clusters keep combining characters and emoji sequences together.
pub(super) fn editor_layout(input: &str, cursor: usize, width: u16) -> EditorLayout {
    debug_assert!(cursor <= input.len() && input.is_char_boundary(cursor));
    let (cursor_column, cursor_row) = wrapped_lines(&input[..cursor], width, true);
    let (_, _, lines) = wrapped_lines_with_content(input, width, true);

    EditorLayout {
        lines,
        cursor_column,
        cursor_row,
    }
}

fn wrapped_lines(input: &str, width: u16, trailing_cursor_row: bool) -> (usize, usize) {
    let (column, row, _) = wrapped_lines_with_content(input, width, trailing_cursor_row);
    (column, row)
}

fn wrapped_lines_with_content(
    input: &str,
    width: u16,
    trailing_cursor_row: bool,
) -> (usize, usize, Vec<String>) {
    let width = usize::from(width.max(1));
    let mut lines = vec![String::new()];
    let mut column: usize = 0;

    for grapheme in input.graphemes(true) {
        if grapheme == "\n" {
            lines.push(String::new());
            column = 0;
            continue;
        }

        let grapheme_width = grapheme.width();
        if grapheme_width > 0 && column > 0 && column.saturating_add(grapheme_width) > width {
            lines.push(String::new());
            column = 0;
        }
        lines
            .last_mut()
            .expect("editor always has a line")
            .push_str(grapheme);
        column = column.saturating_add(grapheme_width);
    }

    // Once the final cell is occupied, the insertion cursor belongs at the
    // beginning of the next visual row.
    if trailing_cursor_row && column >= width {
        lines.push(String::new());
        column = 0;
    }

    (column, lines.len() - 1, lines)
}
