use std::{
    io::{self, Write},
    usize,
};

use color_print::{cwrite, cwriteln};
use crossterm::{
    ExecutableCommand, cursor,
    event::{Event, KeyCode, KeyEvent, KeyModifiers, read},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, enable_raw_mode},
};

use crate::{
    grammar,
    interface::Interface,
    parsec::{
        display::{format_ast, format_messages_with_source},
        msg::ParserMessages,
        tree::{RedNode, TreeAllocRef},
    },
    runtime::{self, Payload},
    scheme::layers::{NodePath, ParseTreeIR, ParseTreeQuery},
};

pub struct CliInterface {
    ged: runtime::GlobalEventDispatcher,
    grammar: &'static grammar::Grammar,
}

impl Interface for CliInterface {
    fn new(ged: runtime::GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self {
        Self { ged, grammar }
    }
    fn ged(&self) -> &runtime::GlobalEventDispatcher {
        &self.ged
    }
}

// RedGreenTreeIR is always the first downstream layer (SourceText → pass[0] → CST).
const TREE_LAYER: fn() -> runtime::RuntimePath = || runtime::RuntimePath(vec![0]);

impl CliInterface {
    pub fn run(&self) -> io::Result<()> {
        let mut stdout = io::stdout();

        enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;

        print_greeting(&mut stdout)?;
        write!(stdout, "input: \r\n")?;
        stdout.flush()?;

        // Record the row where line 1 starts, so we can do absolute redraws
        let (_, origin_row) = cursor::position()?;

        let mut state = EditorState {
            buffer: String::new(),
            cursor_pos: 0,
            origin_row,
        };

        // Draw initial line marker
        full_redraw(&mut stdout, &mut state)?;

        loop {
            match read()? {
                Event::Key(key) => {
                    if !key_event_handler(&self, &mut stdout, &mut state, key)? {
                        break;
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn query_tree<T: 'static>(
        &self,
        revision: runtime::RevisionId,
        index: ParseTreeQuery,
    ) -> runtime::RuntimeResult<T>
    where
        T: Clone,
    {
        let value: Payload =
            self.query_layer::<ParseTreeIR>(TREE_LAYER(), revision.into(), index)?;
        value
            .downcast_ref::<T>()
            .cloned()
            .ok_or_else(|| runtime::RuntimeError::InvalidRequest {
                message: format!("query result had unexpected payload type"),
            })
    }

    fn display_result(
        &self,
        source: &str,
        revision: runtime::RevisionId,
    ) -> runtime::RuntimeResult<(String, String)> {
        let messages: ParserMessages = self.query_tree(revision, ParseTreeQuery::Message)?;
        let alloc: TreeAllocRef = self.query_tree(revision, ParseTreeQuery::Allocator)?;
        let root_id: usize = self.query_tree(revision, ParseTreeQuery::Path(NodePath::root()))?;

        let message_text = format_messages_with_source(self.grammar, &messages, source);
        let ast_text = format_ast(self.grammar, &RedNode::root(root_id), &alloc, source);
        Ok((message_text, ast_text))
    }
}

// Width of the line marker, e.g. "   1 | " = 7 chars
const MARKER_WIDTH: u16 = 7;

struct EditorState {
    buffer: String,
    cursor_pos: usize,
    origin_row: u16,
}

impl EditorState {
    /// Returns (line_index, col_within_line) for the current cursor_pos (0-indexed)
    fn cursor_line_col(&self) -> (usize, usize) {
        buf_line_col(&self.buffer, self.cursor_pos)
    }
}

/// Returns (line_index, col) for a buffer position (both 0-indexed)
fn buf_line_col(buffer: &str, pos: usize) -> (usize, usize) {
    let before = &buffer[..pos];
    let line = before.matches('\n').count();
    let col = before.rfind('\n').map(|p| pos - p - 1).unwrap_or(pos);
    (line, col)
}

/// Start of the line containing pos
fn line_start(buffer: &str, pos: usize) -> usize {
    buffer[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0)
}

/// End of the line containing pos (index of '\n' or buffer.len())
fn line_end(buffer: &str, pos: usize) -> usize {
    buffer[pos..]
        .find('\n')
        .map(|p| pos + p)
        .unwrap_or(buffer.len())
}

fn print_greeting(stdout: &mut io::Stdout) -> io::Result<()> {
    writeln!(stdout, "----------------------------------\r")?;
    cwriteln!(stdout, "> <green>Grammax CLI</green>\r")?;
    cwriteln!(stdout, "> Press <green>Ctrl + C</green> to exit.\r")?;
    cwriteln!(stdout, "> Press <green>Ctrl + S</green> to submit.\r")?;
    writeln!(stdout, "----------------------------------\r")?;
    Ok(())
}

#[inline]
fn move_to(stdout: &mut io::Stdout, col: u16, row: u16) -> io::Result<()> {
    write!(stdout, "\x1b[{};{}H", row + 1, col + 1)
}

fn full_redraw(stdout: &mut io::Stdout, state: &mut EditorState) -> io::Result<()> {
    // Hide cursor during redraw to prevent flickering
    write!(stdout, "\x1b[?25l")?;

    // Move to origin then erase to end of display
    move_to(stdout, 0, state.origin_row)?;
    write!(stdout, "\x1b[J")?;

    // Draw every line
    let lines: Vec<&str> = state.buffer.split('\n').collect();
    let num_lines = lines.len();
    for (i, line) in lines.iter().enumerate() {
        cwrite!(stdout, "<black!>{:>4} | </>", i + 1)?;
        write!(stdout, "{}", line)?;
        if i < num_lines - 1 {
            write!(stdout, "\r\n")?;
        }
    }
    stdout.flush()?;

    // Query real cursor position to detect terminal scrolling.
    // After drawing, cursor is at the end of the last line.
    let (_, actual_row) = cursor::position()?;
    // origin_row should be actual_row - (num_lines - 1)
    let expected_last_row = state.origin_row + (num_lines as u16 - 1);
    if actual_row != expected_last_row {
        // Terminal scrolled: recalibrate
        state.origin_row = actual_row.saturating_sub(num_lines as u16 - 1);
    }

    // Reposition cursor, then show it
    let (line_idx, col) = state.cursor_line_col();
    move_to(
        stdout,
        MARKER_WIDTH + col as u16,
        state.origin_row + line_idx as u16,
    )?;
    write!(stdout, "\x1b[?25h")?;

    stdout.flush()
}

fn key_event_handler(
    cli: &CliInterface,
    stdout: &mut io::Stdout,
    state: &mut EditorState,
    event: KeyEvent,
) -> io::Result<bool> {
    match event.code {
        KeyCode::Char(c) => {
            if event.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                stdout.execute(LeaveAlternateScreen)?;
                return Ok(false);
            }

            if event.modifiers.contains(KeyModifiers::CONTROL) && c == 's' {
                // Submit to runtime and display results
                let source = state.buffer.clone();
                let settled = cli.input(0, usize::MAX, &source);

                let num_lines = state.buffer.matches('\n').count() as u16 + 1;
                move_to(stdout, 0, state.origin_row + num_lines)?;
                write!(stdout, "\r\n")?;
                writeln!(stdout, "----------------------------------\r")?;
                match settled.and_then(|revision| cli.display_result(&source, revision)) {
                    Ok((messages, ast)) => {
                        if !messages.is_empty() {
                            cwrite!(stdout, "\r\n<bold>Messages:</bold>\r\n")?;
                            for line in messages.lines() {
                                write!(stdout, "{}\r\n", line)?;
                            }
                            write!(stdout, "\r\n")?;
                        }
                        cwrite!(stdout, "<bold>AST:</bold>\r\n")?;
                        for line in ast.lines() {
                            write!(stdout, "{}\r\n", line)?;
                        }
                        write!(stdout, "\r\n")?;
                    }
                    Err(e) => {
                        write!(stdout, "Error: {:?}\r\n", e)?;
                    }
                }
                writeln!(stdout, "----------------------------------\r")?;
                stdout.flush()?;

                // Reset state: new origin is after the output
                let (_, _new_row) = cursor::position()?;
                write!(stdout, "input: \r\n")?;
                stdout.flush()?;
                let (_, origin_row) = cursor::position()?;
                state.buffer.clear();
                state.cursor_pos = 0;
                state.origin_row = origin_row;
                full_redraw(stdout, state)?;
                return Ok(true);
            }

            state.buffer.insert(state.cursor_pos, c);
            state.cursor_pos += 1;
            full_redraw(stdout, state)?;
        }

        KeyCode::Backspace => {
            if state.cursor_pos > 0 {
                state.cursor_pos -= 1;
                state.buffer.remove(state.cursor_pos);
                full_redraw(stdout, state)?;
            }
        }

        KeyCode::Enter => {
            state.buffer.insert(state.cursor_pos, '\n');
            state.cursor_pos += 1;
            full_redraw(stdout, state)?;
        }

        KeyCode::Left => {
            let ls = line_start(&state.buffer, state.cursor_pos);
            if state.cursor_pos > ls {
                state.cursor_pos -= 1;
                let (line_idx, col) = state.cursor_line_col();
                move_to(
                    stdout,
                    MARKER_WIDTH + col as u16,
                    state.origin_row + line_idx as u16,
                )?;
                stdout.flush()?;
            }
        }

        KeyCode::Right => {
            let le = line_end(&state.buffer, state.cursor_pos);
            if state.cursor_pos < le {
                state.cursor_pos += 1;
                let (line_idx, col) = state.cursor_line_col();
                move_to(
                    stdout,
                    MARKER_WIDTH + col as u16,
                    state.origin_row + line_idx as u16,
                )?;
                stdout.flush()?;
            }
        }

        KeyCode::Up => {
            let (line_idx, col) = state.cursor_line_col();
            if line_idx > 0 {
                let cur_ls = line_start(&state.buffer, state.cursor_pos);
                let prev_ls = line_start(&state.buffer, cur_ls - 1);
                let prev_le = line_end(&state.buffer, prev_ls);
                let prev_len = prev_le - prev_ls;
                let new_col = col.min(prev_len);
                state.cursor_pos = prev_ls + new_col;
                move_to(
                    stdout,
                    MARKER_WIDTH + new_col as u16,
                    state.origin_row + (line_idx - 1) as u16,
                )?;
                stdout.flush()?;
            }
        }

        KeyCode::Down => {
            let (line_idx, col) = state.cursor_line_col();
            let le = line_end(&state.buffer, state.cursor_pos);
            if le < state.buffer.len() {
                let next_ls = le + 1;
                let next_le = line_end(&state.buffer, next_ls);
                let next_len = next_le - next_ls;
                let new_col = col.min(next_len);
                state.cursor_pos = next_ls + new_col;
                move_to(
                    stdout,
                    MARKER_WIDTH + new_col as u16,
                    state.origin_row + (line_idx + 1) as u16,
                )?;
                stdout.flush()?;
            }
        }

        _ => {}
    }
    Ok(true)
}

#[cfg(test)]
mod tests {

    use crate::{
        new_grammar,
        parsec::words::{EndOfInput, NUMS},
        runtime::{CompilerBuilder, ComposedCompiler, ParserPass, RuntimeService},
        scheme::layers::ParseTreeIR,
    };

    use super::*;

    #[test]
    fn test_cli_interface() {
        let grammar = new_grammar!(
            start where
            start -> r!(expr) + tt(EndOfInput)
            expr -> r!(add) | r!(mul) | r!(primary)
            add  -> field("lhs:", r!(expr)) + tt("+") + field("rhs:", r!(expr).drop(1))
            mul  -> field("lhs:", r!(expr).drop(1)) + tt("*") + field("rhs:", r!(expr).drop(2))
            primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
        );

        let (pass, _observer) = CompilerBuilder::new()
            .then_pass(ParserPass::new(grammar))
            .then_layer(ParseTreeIR::default())
            .tap();

        let runtime = RuntimeService::<CliInterface>::new(grammar, move |evt_tx| {
            ComposedCompiler::from_pass_with_events(pass, evt_tx)
        });

        runtime.run().expect("runtime failed");
    }
}
