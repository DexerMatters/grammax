use std::{
    io::{self, Write},
    marker::PhantomData,
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
        tree::RedNode,
    },
    runtime::{
        self,
        compiler::{ContainsPath, Down, Here, TypedTree},
        dispatcher::GlobalEventDispatcher,
    },
    utils,
    scheme::layers::{NodePath, ParseTreeIR, ParseTreeQuery, ParseTreeValue, SourceText},
};

pub struct CliInterface<Tree: TypedTree> {
    ged: GlobalEventDispatcher,
    grammar: &'static grammar::Grammar,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree> Interface<Tree> for CliInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText> + ContainsPath<Down<Here>, Target = ParseTreeIR>,
{
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self {
        Self {
            ged,
            grammar,
            _marker: PhantomData,
        }
    }
    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }
}

impl<Tree> CliInterface<Tree>
where
    Tree: TypedTree
        + ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
{
    pub fn run(&self) -> io::Result<()> {
        let mut stdout = io::stdout();

        enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;

        print_greeting(&mut stdout)?;
        write!(stdout, "input: \r\n")?;
        stdout.flush()?;

        let (_, origin_row) = cursor::position()?;

        let mut state = EditorState {
            buffer: String::new(),
            cursor_pos: 0,
            origin_row,
        };

        full_redraw(&mut stdout, &mut state)?;

        loop {
            match read()? {
                Event::Key(key) => {
                    if !self.key_event_handler(&mut stdout, &mut state, key)? {
                        break;
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn display_result(
        &self,
        source: &str,
        revision: runtime::RevisionId,
    ) -> runtime::RuntimeResult<(String, String)> {
        let rev = Some(revision);

        let messages = match self.query_layer::<Down<Here>>(rev, ParseTreeQuery::Message)? {
            ParseTreeValue::Messages(m) => m,
            other => {
                return Err(runtime::RuntimeError::UndefinedBehavior {
                    message: format!("expected Messages, got {other:?}"),
                });
            }
        };
        let alloc = match self.query_layer::<Down<Here>>(rev, ParseTreeQuery::Allocator)? {
            ParseTreeValue::Allocator(a) => a,
            other => {
                return Err(runtime::RuntimeError::UndefinedBehavior {
                    message: format!("expected Allocator, got {other:?}"),
                });
            }
        };
        let root_id =
            match self.query_layer::<Down<Here>>(rev, ParseTreeQuery::Path(NodePath::root()))? {
                ParseTreeValue::GreenId(id) => id,
                other => {
                    return Err(runtime::RuntimeError::UndefinedBehavior {
                        message: format!("expected GreenId, got {other:?}"),
                    });
                }
            };

        let message_text = format_messages_with_source(self.grammar, &messages, source);
        let ast_text = format_ast(self.grammar, &RedNode::root(root_id), &alloc, source);
        Ok((message_text, ast_text))
    }

    fn key_event_handler(
        &self,
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
                    let source = state.buffer.clone();
                    let settled = <Self as Interface<Tree>>::edit_source_text(
                        self,
                        utils::Range::new(
                            utils::Position::zero(),
                            utils::Position::new(usize::MAX, usize::MAX),
                        ),
                        &source,
                    );

                    let num_lines = state.buffer.matches('\n').count() as u16 + 1;
                    move_to(stdout, 0, state.origin_row + num_lines)?;
                    write!(stdout, "\r\n")?;
                    writeln!(stdout, "----------------------------------\r")?;
                    match settled.and_then(|revision| self.display_result(&source, revision)) {
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

// #[cfg(test)]
// mod tests {

//     use crate::{
//         grammar::Grammar,
//         runtime::{BuildTree, CompilerBuilder, Down, End, Here, Observe, ParserPass, Then},
//         scheme::layers::{ParseTreeIR, SourceText},
//     };

//     use super::*;

//     type ParseTreePass = Then<SourceText, ParserPass, End<ParseTreeIR>>;

//     #[test]
//     fn test_cli_interface() {
//         let grammar =
//             Grammar::load_from("/home/dexer/repos/grammax/target/debug/json.gmx.bin").unwrap();

//         let pass: ParseTreePass =
//             CompilerBuilder::new().then(ParserPass::new(grammar), ParseTreeIR::default());
//         let _observer = pass.observe::<Down<Here>>();

//         let runtime = pass.build_runtime::<CliInterface<ParseTreePass>>(grammar);

//         runtime.run().expect("runtime failed");
//     }
// }
