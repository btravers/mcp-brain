//! Full-screen local review. Only an explicit Apply action can move a note.
use anyhow::{Context, Result, ensure};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
};
use mcp_brain::{
    note::Note,
    review::{Decision, Plan, ReviewSession},
    undo::UndoPlan,
    vault::{Outcome, Vault, category},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use std::{
    io,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use crate::terminal::safe;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Note,
    Changes,
    Summary,
    Undo,
}

enum Input {
    Comment(String),
    Find(String),
}

type Application = Result<Outcome>;

#[derive(Clone, Copy)]
enum SaveAction {
    Apply(usize),
    Undo(usize),
}

struct SaveJob {
    action: SaveAction,
    receiver: Receiver<Application>,
    handle: thread::JoinHandle<()>,
}

#[derive(Clone)]
struct MarkdownTheme;

impl tui_markdown::StyleSheet for MarkdownTheme {
    fn heading(&self, level: u8) -> Style {
        let style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        if level == 1 {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        }
    }
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }
    fn code_block_fence(&self) -> &str {
        ""
    }
    fn code(&self) -> Style {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::DIM)
    }
}

struct App {
    vault: Vault,
    plan: Plan,
    session: ReviewSession,
    index: usize,
    list: ListState,
    view: View,
    scroll: usize,
    max_scroll: usize,
    viewport: usize,
    total_lines: usize,
    preview: Option<Plan>,
    undo_preview: Option<UndoPlan>,
    undo_error: Option<String>,
    notice: String,
    error: Option<String>,
    input: Option<Input>,
    help: bool,
    worker: Option<SaveJob>,
    quit_after_save: bool,
    sidebar: Rect,
    buttons: Vec<(Rect, char)>,
    color: bool,
    usable: bool,
}

impl App {
    fn new(vault: Vault, id: &str, color: bool) -> Result<Self> {
        let plan = vault.load_plan(id)?;
        let session = vault.review_session(id)?;
        ensure!(!plan.moves.is_empty(), "plan has no review items");
        let index = session
            .items
            .iter()
            .position(|i| matches!(i.decision, Decision::Pending | Decision::Accepted))
            .unwrap_or(0);
        let mut app = Self {
            vault,
            plan,
            session,
            index,
            list: ListState::default(),
            view: View::Note,
            scroll: 0,
            max_scroll: 0,
            viewport: 0,
            total_lines: 0,
            preview: None,
            undo_preview: None,
            undo_error: None,
            notice: "Apply now saves this move immediately. Drop leaves the note in Inbox.".into(),
            error: None,
            input: None,
            help: false,
            worker: None,
            quit_after_save: false,
            sidebar: Rect::default(),
            buttons: Vec::new(),
            color,
            usable: true,
        };
        app.prepare();
        Ok(app)
    }

    fn accent(&self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color {
            style.fg(Color::Cyan)
        } else {
            style
        }
    }

    fn prepare(&mut self) {
        self.list.select(Some(self.index));
        self.scroll = 0;
        self.preview = None;
        self.error = None;
        self.refresh_undo();
        if self.session.items[self.index].decision != Decision::Applied {
            match self.vault.review_item_plan(&self.plan.id, self.index) {
                Ok(plan) => self.preview = Some(plan),
                Err(error) => {
                    self.error = Some(format!(
                        "Apply unavailable: {error:#}. Leave a comment or request a fresh proposal."
                    ))
                }
            }
        }
    }

    fn refresh_undo(&mut self) {
        match self.vault.review_undo_plan(&self.plan.id) {
            Ok(undo) => {
                self.undo_preview = Some(undo);
                self.undo_error = None;
            }
            Err(error) => {
                self.undo_preview = None;
                self.undo_error = Some(format!("Undo unavailable: {error:#}"));
            }
        }
    }

    fn select(&mut self, index: usize) {
        if self.worker.is_some() {
            return;
        }
        self.index = index.min(self.plan.moves.len() - 1);
        self.prepare();
    }

    fn advance(&mut self) {
        let count = self.plan.moves.len();
        if let Some(next) = (1..=count)
            .map(|step| (self.index + step) % count)
            .find(|i| {
                matches!(
                    self.session.items[*i].decision,
                    Decision::Pending | Decision::Accepted
                )
            })
        {
            self.select(next);
        } else {
            self.preview = None;
            self.refresh_undo();
            self.view = View::Summary;
            self.scroll = 0;
            self.notice = "Review complete. Applied moves are saved; dropped and commented notes stay in Inbox. Press q to close.".into();
        }
    }

    fn current_note(&self) -> String {
        let item = &self.plan.moves[self.index];
        let current = self.preview.as_ref().unwrap_or(&self.plan);
        current
            .changes
            .iter()
            .find(|c| c.path == item.source)
            .and_then(|c| c.before.clone())
            .unwrap_or_default()
    }

    fn overview(&self) -> String {
        let source = self.current_note();
        let body = Note::parse(&source)
            .map(|n| n.body().to_owned())
            .unwrap_or(source);
        let paragraph = body
            .split("\n\n")
            .find(|part| {
                let part = part.trim();
                !part.is_empty() && !part.starts_with(['#', '`', '<', '|', '-'])
            })
            .unwrap_or("The source note is shown below.")
            .trim();
        let mut text: String = paragraph.chars().take(400).collect();
        if paragraph.chars().count() > 400 {
            text.push('…');
        }
        text
    }

    fn document(&self) -> String {
        match self.view {
            View::Note => {
                let source = self.current_note();
                let body = Note::parse(&source)
                    .map(|n| n.body().to_owned())
                    .unwrap_or(source);
                format!("## Summary\n\n{}\n\n---\n\n{body}", self.overview())
            }
            View::Changes => {
                let Some(plan) = &self.preview else {
                    return "# Changes\n\nThis item has already been applied or needs a fresh proposal. Its recorded decision is available in Review summary.".into();
                };
                let mut text = format!(
                    "# Exact changes for this move\n\n{} files are affected, including index, parent metadata and link maintenance.\n",
                    plan.changes.len()
                );
                for change in &plan.changes {
                    // Render snapshots as literal text, never as Markdown instructions.
                    text.push_str(&format!("\n## {}\n", change.path));
                    if let Some(before) = &change.before {
                        text.push_str("\nBefore\n\n");
                        for line in before.split('\n') {
                            text.push_str(&format!("    - {line}\n"));
                        }
                    }
                    if let Some(after) = &change.after {
                        text.push_str("\nAfter\n\n");
                        for line in after.split('\n') {
                            text.push_str(&format!("    + {line}\n"));
                        }
                    }
                }
                text
            }
            View::Summary => {
                let mut text = "# Review summary\n\nApplied items are already saved. Dropped proposals do not delete notes. Comments remain available to the assistant through inbox_plan_review.\n\n".to_owned();
                if let Some(error) = &self.error {
                    text.push_str(&format!("## Attention\n\n{error}\n\n"));
                }
                for (index, (item, review)) in
                    self.plan.moves.iter().zip(&self.session.items).enumerate()
                {
                    text.push_str(&format!(
                        "## {}. {} · {}\n\nSuggested destination: {}\n\nWhy: {}\n\n",
                        index + 1,
                        title(&item.source),
                        decision(review.decision),
                        item.destination,
                        item.reason
                    ));
                    if let Some(comment) = &review.comment {
                        text.push_str(&format!("Your comment: {comment}\n\n"));
                    }
                }
                text
            }
            View::Undo => {
                let Some(undo) = &self.undo_preview else {
                    return self
                        .undo_error
                        .clone()
                        .unwrap_or_else(|| "No move is available to undo.".into());
                };
                let mut text = format!(
                    "# Undo last move\n\nReturn **{}** to Inbox and restore its previous metadata, indexes and links.\n\n{} related files will change. Generated indexes are archived, never deleted. Press **u** to undo this exact save.\n",
                    title(&self.plan.moves[undo.index].source),
                    undo.changes.len()
                );
                for change in &undo.changes {
                    text.push_str(&format!("\n## {}\n", change.path));
                    if let Some(before) = &change.before {
                        text.push_str("\nBefore\n\n");
                        for line in before.split('\n') {
                            text.push_str(&format!("    - {line}\n"));
                        }
                    }
                    if let Some(after) = &change.after {
                        text.push_str("\nAfter\n\n");
                        for line in after.split('\n') {
                            text.push_str(&format!("    + {line}\n"));
                        }
                    }
                }
                text
            }
        }
    }

    fn apply(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let Some(prepared) = &self.preview else {
            return;
        };
        let vault = self.vault.clone();
        let id = self.plan.id.clone();
        let prepared_id = prepared.id.clone();
        let index = self.index;
        self.notice = format!(
            "Saving {}… The move continues safely if you request exit.",
            title(&self.plan.moves[index].source)
        );
        self.start_save(SaveAction::Apply(index), move || {
            vault.apply_review_item(&id, index, &prepared_id)
        });
    }

    fn undo(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let Some(undo) = &self.undo_preview else {
            self.error = self.undo_error.clone();
            return;
        };
        let index = undo.index;
        let undo_id = undo.id.clone();
        let vault = self.vault.clone();
        let id = self.plan.id.clone();
        self.notice = format!("Undoing {}…", title(&self.plan.moves[index].source));
        self.start_save(SaveAction::Undo(index), move || {
            vault.undo_review_item(&id, &undo_id)
        });
    }

    fn start_save(
        &mut self,
        action: SaveAction,
        save: impl FnOnce() -> Application + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = sender.send(save());
        });
        self.worker = Some(SaveJob {
            action,
            receiver,
            handle,
        });
    }

    fn poll_application(&mut self) -> Result<bool> {
        let Some(job) = &self.worker else {
            return Ok(false);
        };
        let action = job.action;
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(false),
            Err(TryRecvError::Disconnected) => Err(anyhow::anyhow!(
                "move interrupted; inspect sync-status and recovery before retrying"
            )),
        };
        if let Some(job) = self.worker.take() {
            let _ = job.handle.join();
        }
        self.session = self.vault.review_session(&self.plan.id)?;
        match result {
            Ok(outcome) => {
                match action {
                    SaveAction::Apply(index) => {
                        self.notice = format!(
                            "Applied {}. Saved and advanced to the next item.",
                            title(&self.plan.moves[index].source)
                        );
                        self.advance();
                    }
                    SaveAction::Undo(index) => {
                        self.index = index;
                        self.view = View::Note;
                        self.prepare();
                        self.notice = format!(
                            "Undid {}. The note is back in Inbox; comments are preserved.",
                            title(&self.plan.moves[index].source)
                        );
                    }
                }
                if let Some(warning) = outcome.warning {
                    self.error = Some(warning);
                }
            }
            Err(error) => {
                self.prepare();
                self.error = Some(format!("{error:#}. Check the saved state before retrying."));
            }
        }
        Ok(self.quit_after_save)
    }

    fn save_comment(&mut self, text: String) -> Result<()> {
        self.session =
            self.vault
                .record_review(&self.plan.id, self.index, Decision::Commented, Some(text))?;
        self.notice = "Comment saved for the assistant. The note stays in Inbox.".into();
        self.advance();
        Ok(())
    }

    fn action(&mut self, key: char) -> Result<bool> {
        if self.worker.is_some() {
            if key == 'q' {
                self.quit_after_save = true;
                self.notice = "Finishing the current save before closing…".into();
            }
            return Ok(false);
        }
        match key {
            'a' => self.apply(),
            'u' => self.undo(),
            'U' => {
                self.view = View::Undo;
                self.scroll = 0;
            }
            'd' => {
                self.session =
                    self.vault
                        .record_review(&self.plan.id, self.index, Decision::Dropped, None)?;
                self.notice = "Proposal dropped. The note stays in Inbox.".into();
                self.advance();
            }
            'c' if self.session.items[self.index].decision != Decision::Applied => {
                self.input = Some(Input::Comment(
                    self.session.items[self.index]
                        .comment
                        .clone()
                        .unwrap_or_default(),
                ));
            }
            '/' => self.input = Some(Input::Find(String::new())),
            'n' => self.select((self.index + 1) % self.plan.moves.len()),
            'p' => self.select(if self.index == 0 {
                self.plan.moves.len() - 1
            } else {
                self.index - 1
            }),
            'f' => self.advance(),
            's' => {
                self.view = View::Summary;
                self.scroll = 0;
            }
            'v' => {
                self.view = View::Changes;
                self.scroll = 0;
            }
            '1' => {
                self.view = View::Note;
                self.scroll = 0;
            }
            '?' => self.help = !self.help,
            'q' => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    fn event(&mut self, event: Event) -> Result<bool> {
        if !self.usable {
            if let Event::Key(key) = event
                && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
            {
                return self.action('q');
            }
            return Ok(false);
        }
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Release {
                return Ok(false);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return self.action('q');
            }
            if self.input.is_some() {
                match key.code {
                    KeyCode::Esc => self.input = None,
                    KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
                        match self.input.take().unwrap() {
                            Input::Comment(text) if !text.trim().is_empty() => {
                                self.save_comment(text)?
                            }
                            Input::Find(text) if !text.trim().is_empty() => {
                                let query = text.to_lowercase();
                                let found = (1..=self.plan.moves.len())
                                    .map(|step| (self.index + step) % self.plan.moves.len())
                                    .find(|i| {
                                        format!(
                                            "{} {}",
                                            self.plan.moves[*i].source, self.plan.moves[*i].reason
                                        )
                                        .to_lowercase()
                                        .contains(&query)
                                    });
                                if let Some(index) = found {
                                    self.select(index);
                                } else {
                                    self.notice = format!("No proposal matches {text}.");
                                }
                            }
                            _ => {}
                        }
                    }
                    KeyCode::Backspace => {
                        self.input_text().pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.insert(&c.to_string())
                    }
                    KeyCode::Enter => self.insert("\n"),
                    _ => {}
                }
                return Ok(false);
            }
            if self.help {
                self.help = false;
                return Ok(false);
            }
            match key.code {
                KeyCode::Esc => return self.action('q'),
                KeyCode::Right => self.action('n').map(|_| ())?,
                KeyCode::Left => self.action('p').map(|_| ())?,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll = self.scroll.saturating_add(1).min(self.max_scroll)
                }
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    self.scroll = self
                        .scroll
                        .saturating_add(self.viewport.saturating_sub(1))
                        .min(self.max_scroll)
                }
                KeyCode::PageUp => {
                    self.scroll = self.scroll.saturating_sub(self.viewport.saturating_sub(1))
                }
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = self.max_scroll,
                KeyCode::Tab => {
                    self.view = match self.view {
                        View::Note => View::Changes,
                        View::Changes => View::Summary,
                        View::Summary => View::Note,
                        View::Undo => View::Note,
                    };
                    self.scroll = 0;
                }
                KeyCode::Char(c)
                    if key.kind == KeyEventKind::Press
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    return self.action(c);
                }
                _ => {}
            }
        } else if let Event::Mouse(mouse) = event {
            if self.input.is_some() || self.help {
                return Ok(false);
            }
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.scroll = self.scroll.saturating_add(3).min(self.max_scroll)
                }
                MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
                MouseEventKind::Down(event::MouseButton::Left) => {
                    if self.sidebar.contains((mouse.column, mouse.row).into()) {
                        let index =
                            self.list.offset() + mouse.row.saturating_sub(self.sidebar.y) as usize;
                        self.select(index);
                    } else if let Some((_, action)) = self
                        .buttons
                        .iter()
                        .find(|(r, _)| r.contains((mouse.column, mouse.row).into()))
                    {
                        return self.action(*action);
                    }
                }
                _ => {}
            }
        } else if let Event::Paste(text) = event
            && self.input.is_some()
        {
            self.insert(&text);
        }
        Ok(false)
    }

    fn input_text(&mut self) -> &mut String {
        match self.input.as_mut().unwrap() {
            Input::Comment(s) | Input::Find(s) => s,
        }
    }

    fn insert(&mut self, text: &str) {
        let current = self.input_text();
        let text = safe(text);
        if current.len() + text.len() <= 4000 {
            current.push_str(&text);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 50 || area.height < 18 {
            self.usable = false;
            self.buttons.clear();
            self.sidebar = Rect::default();
            frame.render_widget(
                Paragraph::new("Enlarge the terminal to at least 50 × 18. Press q to exit.")
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        self.usable = true;
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);
        let applied = self
            .session
            .items
            .iter()
            .filter(|r| r.decision == Decision::Applied)
            .count();
        let dropped = self
            .session
            .items
            .iter()
            .filter(|r| r.decision == Decision::Dropped)
            .count();
        let comments = self
            .session
            .items
            .iter()
            .filter(|r| r.decision == Decision::Commented)
            .count();
        frame.render_widget(
            Paragraph::new(format!(
                "  Item {} / {}     {} applied   {} dropped   {} need precision",
                self.index + 1,
                self.plan.moves.len(),
                applied,
                dropped,
                comments
            ))
            .style(self.accent())
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .title(" INBOX REVIEW "),
            ),
            rows[0],
        );
        let content = if area.width >= 100 {
            let panes =
                Layout::horizontal([Constraint::Length(30), Constraint::Min(40)]).split(rows[1]);
            let block = Block::bordered().title(" Review queue · ← → ");
            self.sidebar = block.inner(panes[0]);
            let items: Vec<_> = self
                .plan
                .moves
                .iter()
                .zip(&self.session.items)
                .map(|(item, r)| {
                    let marker = match r.decision {
                        Decision::Applied => "+",
                        Decision::Dropped => "-",
                        Decision::Commented => "?",
                        Decision::Accepted => "~",
                        Decision::Pending => " ",
                    };
                    ListItem::new(format!("[{marker}] {}", safe(title(&item.source))))
                })
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(self.accent().add_modifier(Modifier::REVERSED)),
                panes[0],
                &mut self.list,
            );
            panes[1]
        } else {
            self.sidebar = Rect::default();
            rows[1]
        };
        let details = Layout::vertical([Constraint::Length(7), Constraint::Min(4)]).split(content);
        let item = &self.plan.moves[self.index];
        let suggestion = Text::from(vec![
            Line::from(vec![
                Span::styled("Move to  ", self.accent()),
                Span::raw(safe(&item.destination)),
            ]),
            Line::from(vec![
                Span::styled("Effects  ", self.accent()),
                Span::raw(format!(
                    "Move this note + {} related file updates · v details",
                    self.preview
                        .as_ref()
                        .map(|p| p.changes.len().saturating_sub(2))
                        .unwrap_or(0)
                )),
            ]),
            Line::from(vec![
                Span::styled("Why      ", self.accent()),
                Span::raw(safe(&item.reason)),
            ]),
        ]);
        frame.render_widget(
            Paragraph::new(suggestion).wrap(Wrap { trim: false }).block(
                Block::bordered().border_style(self.accent()).title(format!(
                    " SUGGESTION · {} · {} ",
                    category(&item.destination).unwrap_or("note"),
                    decision(self.session.items[self.index].decision)
                )),
            ),
            details[0],
        );
        let document = safe(&self.document());
        let options = tui_markdown::Options::new(MarkdownTheme)
            .table_width(details[1].width.saturating_sub(3));
        let mut text = tui_markdown::from_str_with_options(&document, &options);
        if !self.color {
            text.style = Style::default();
            for line in &mut text.lines {
                line.style = Style::default();
                for span in &mut line.spans {
                    span.style = Style::default().add_modifier(
                        span.style.add_modifier
                            & (Modifier::BOLD | Modifier::UNDERLINED | Modifier::ITALIC),
                    );
                }
            }
        }
        let title = match self.view {
            View::Note => " NOTE · Tab: Changes / Review summary ",
            View::Changes => " EXACT CHANGES · Tab: Review summary / Note ",
            View::Summary => " REVIEW SUMMARY · Tab: Note / Changes ",
            View::Undo => " UNDO LAST MOVE · u to undo · Tab: Note ",
        };
        let block = Block::bordered().title(title);
        let inside = block.inner(details[1]);
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        self.viewport = inside.height as usize;
        self.total_lines = paragraph.line_count(inside.width.saturating_sub(1));
        self.max_scroll = self
            .total_lines
            .saturating_sub(self.viewport)
            .min(u16::MAX as usize);
        self.scroll = self.scroll.min(self.max_scroll);
        frame.render_widget(block, details[1]);
        let text_area = Rect {
            width: inside.width.saturating_sub(1),
            ..inside
        };
        frame.render_widget(paragraph.scroll((self.scroll as u16, 0)), text_area);
        if self.max_scroll > 0 {
            let mut state = ScrollbarState::new(self.total_lines)
                .position(self.scroll)
                .viewport_content_length(self.viewport);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓")),
                inside,
                &mut state,
            );
        }
        let message = self.error.as_deref().unwrap_or(&self.notice);
        let style = if self.error.is_some() && self.color {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(safe(message))
                .style(style)
                .wrap(Wrap { trim: false }),
            rows[2],
        );
        let labels = if area.width >= 80 {
            vec![
                ('a', " Apply now "),
                ('d', " Drop "),
                ('c', " Comment "),
                ('u', " Undo last "),
                ('s', " Summary "),
                ('q', " Quit "),
            ]
        } else {
            vec![
                ('a', " Apply"),
                ('d', " Drop"),
                ('c', " Comment"),
                ('u', " Undo"),
                ('q', " Quit"),
            ]
        };
        let widths: Vec<_> = labels
            .iter()
            .map(|(_, label)| Constraint::Length((label.len() + 4) as u16))
            .collect();
        let buttons = Layout::horizontal(widths).split(rows[3]);
        self.buttons.clear();
        for ((key, label), rect) in labels.iter().zip(buttons.iter()) {
            let enabled = self.worker.is_none()
                && (key != &'a' || self.preview.is_some())
                && (key != &'u' || self.undo_preview.is_some())
                && (!matches!(key, 'c' | 'd')
                    || self.session.items[self.index].decision != Decision::Applied);
            let style = if !enabled {
                Style::default().add_modifier(Modifier::DIM)
            } else if key == &'a' {
                self.accent().add_modifier(Modifier::REVERSED)
            } else {
                self.accent()
            };
            frame.render_widget(
                Paragraph::new(format!("{key}{label}"))
                    .style(style)
                    .block(Block::bordered()),
                *rect,
            );
            if enabled {
                self.buttons.push((*rect, *key));
            }
        }
        let hint = if area.width >= 100 {
            format!(
                "↑↓ / wheel scroll · PgUp PgDn · ←→ items · / find · ? help      {}–{} / {} lines",
                self.scroll + 1,
                (self.scroll + self.viewport).min(self.total_lines),
                self.total_lines
            )
        } else {
            "↑↓ scroll · ←→ items · Tab views · ? help".into()
        };
        frame.render_widget(Paragraph::new(hint), rows[4]);
        if self.help {
            let rect = modal(area, 72, 16);
            frame.render_widget(Clear, rect);
            frame.render_widget(Paragraph::new("a  Apply this move now and advance\nd  Drop proposal; keep note in Inbox\nc  Comment for the assistant; keep note in Inbox\nu  Undo last save    U  Inspect Undo changes\n← → / p n  Previous / next item\n↑ ↓ / j k / wheel  Scroll note\nPgUp PgDn / Space  Scroll a page\nHome End  Top / bottom\nTab  Note / exact changes / review summary\nf  Next unresolved item     /  Find proposal\ns  Review summary          q / Esc  Save and exit\n\nIn comments: Enter saves, Alt+Enter adds a line, Esc cancels.")
                .block(Block::bordered().title(" Keyboard and mouse ")).wrap(Wrap { trim: false }), rect);
        }
        if let Some(input) = &self.input {
            let rect = modal(area, 78, 10);
            let (title, text) = match input {
                Input::Comment(s) => (" COMMENT · Enter save · Alt+Enter newline · Esc cancel ", s),
                Input::Find(s) => (" FIND PROPOSAL · Enter find · Esc cancel ", s),
            };
            frame.render_widget(Clear, rect);
            let text = format!("{}▏", safe(text));
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            let scroll = paragraph
                .line_count(rect.width.saturating_sub(2))
                .saturating_sub(rect.height.saturating_sub(2) as usize)
                .min(u16::MAX as usize) as u16;
            frame.render_widget(
                paragraph
                    .scroll((scroll, 0))
                    .block(Block::bordered().title(title).border_style(self.accent())),
                rect,
            );
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(job) = self.worker.take() {
            let _ = job.handle.join();
        }
    }
}

fn title(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .strip_suffix(".md")
        .unwrap_or(path)
}

fn decision(decision: Decision) -> &'static str {
    match decision {
        Decision::Pending => "pending",
        Decision::Accepted => "accepted",
        Decision::Dropped => "dropped",
        Decision::Commented => "needs precision",
        Decision::Applied => "applied",
    }
}

fn modal(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
    }
}

pub fn run(vault: Vault, id: &str, color: bool) -> Result<()> {
    let mut app = App::new(vault, id, color)?;
    with_terminal(|terminal| {
        let mut redraw = true;
        loop {
            let was_saving = app.worker.is_some();
            if app.poll_application()? {
                break;
            }
            if was_saving && app.worker.is_none() {
                redraw = true;
            }
            if redraw {
                terminal.draw(|frame| app.draw(frame))?;
                redraw = false;
            }
            if event::poll(Duration::from_millis(100))? {
                let event = event::read()?;
                if matches!(&event, Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved) {
                    continue;
                }
                redraw = true;
                match app.event(event) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => app.error = Some(format!("{error:#}")),
                }
            }
        }
        Ok(())
    })
}

pub(super) fn with_terminal<T>(
    run: impl FnOnce(&mut ratatui::DefaultTerminal) -> Result<T>,
) -> Result<T> {
    ensure!(
        std::env::var("TERM").as_deref() != Ok("dumb"),
        "full-screen review needs a capable terminal; use inbox review PLAN_ID --plain"
    );
    let mut terminal =
        ratatui::try_init().context("cannot initialize full-screen review; use --plain")?;
    let _restore = TerminalRestore;
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    run(&mut terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::{Terminal, backend::TestBackend};
    use std::{fs, time::Instant};

    fn fixture() -> (tempfile::TempDir, App) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        fs::create_dir_all(root.join("0 Inbox")).unwrap();
        let body = format!(
            "---\nauthor: ai\n---\n# Learning ownership\n\nA **bold phrase** and a [reference](https://example.com).\n\n| Name | Purpose |\n| --- | --- |\n| Owner | Lifetime |\n\n```rust\nlet value = 1;\n```\n\n{}\n\nEND OF NOTE\n\n## Notes\n\nHuman annotation stays.\n",
            (0..150)
                .map(|i| format!("Paragraph {i}: useful invented material for scrolling.\n\n"))
                .collect::<String>()
        );
        fs::write(root.join("0 Inbox/Learning.md"), body).unwrap();
        fs::write(root.join("0 Inbox/Gardening.md"), "---\nauthor: ai\n---\n# Gardening\n\nA reference about herbs.\n\n## Notes\n\nHuman suffix.\n").unwrap();
        let vault = Vault::open(&root, Some(temp.path().join("state"))).unwrap();
        let moves: Vec<_> = ["Learning", "Gardening"]
            .map(|name| {
                let source = format!("0 Inbox/{name}.md");
                mcp_brain::review::Move {
                    expected_revision: mcp_brain::note::revision(
                        &vault.read_text(&source).unwrap(),
                    ),
                    source,
                    destination: format!("3 Resources/{name}.md"),
                    reason: "Reusable reference with no ongoing obligation.".into(),
                }
            })
            .into();
        let proposal = vault.propose_plan(moves).unwrap();
        let app = App::new(vault, proposal["id"].as_str().unwrap(), true).unwrap();
        (temp, app)
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let mut line = (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>();
                line.push('\n');
                line
            })
            .collect()
    }

    #[test]
    fn full_screen_formats_markdown_and_scrolls_to_the_end() {
        let (_temp, mut app) = fixture();
        let first = render(&mut app, 130, 38);
        assert!(first.contains("SUGGESTION") && first.contains("Summary"));
        assert!(first.contains("Apply now") && first.contains("Comment"));
        assert!(first.contains("bold phrase") && !first.contains("**bold phrase**"));
        assert!(!first.contains("## Summary") && !first.contains("```rust"));
        assert!(app.max_scroll > 100);
        assert!(first.contains('█'));
        app.event(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)))
            .unwrap();
        let last = render(&mut app, 130, 38);
        assert!(last.contains("END OF NOTE"));
        assert!(last.contains("Human annotation stays."));
    }

    #[test]
    fn narrow_terminals_keep_primary_actions_visible_and_resize_reflows() {
        let (_temp, mut app) = fixture();
        for (width, height) in [(50, 20), (80, 24), (110, 35)] {
            let rendered = render(&mut app, width, height);
            assert!(
                rendered.contains("Apply")
                    && rendered.contains("Drop")
                    && rendered.contains("Comment")
                    && rendered.contains("Quit"),
                "{width}x{height}:\n{rendered}"
            );
        }
        assert!(render(&mut app, 30, 10).contains("Enlarge"));
        app.event(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )))
        .unwrap();
        assert!(
            app.worker.is_none(),
            "an invisible action must not apply a move"
        );
    }

    #[test]
    fn comments_and_drop_are_saved_without_mutating_notes() {
        let (_temp, mut app) = fixture();
        let before = app.vault.snapshot().unwrap();
        app.action('c').unwrap();
        app.event(Event::Paste(
            "This is an ongoing responsibility, not a reference.".into(),
        ))
        .unwrap();
        app.event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )))
        .unwrap();
        assert_eq!(app.session.items[0].decision, Decision::Commented);
        app.action('d').unwrap();
        assert_eq!(app.session.items[1].decision, Decision::Dropped);
        assert_eq!(app.vault.snapshot().unwrap(), before);
        assert!(app.view == View::Summary);
    }

    #[test]
    fn the_apply_action_saves_immediately_without_a_finish_step() {
        let (_temp, mut app) = fixture();
        app.event(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
        )))
        .unwrap();
        assert!(app.worker.is_none());
        app.action('a').unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.worker.is_some() {
            assert!(Instant::now() < deadline, "application did not finish");
            app.poll_application().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(app.session.items[0].decision, Decision::Applied);
        assert_eq!(app.index, 1);
        assert!(app.preview.is_some(), "{:?}", app.error);
        assert!(
            app.vault
                .read_text("3 Resources/Learning.md")
                .unwrap()
                .ends_with("## Notes\n\nHuman annotation stays.\n")
        );
        let resumed = App::new(app.vault.clone(), &app.plan.id, false).unwrap();
        assert_eq!(resumed.index, 1);
        assert_eq!(resumed.session.items[0].decision, Decision::Applied);
    }

    fn finish_save(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.worker.is_some() {
            assert!(Instant::now() < deadline, "save did not finish");
            app.poll_application().unwrap();
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn undo_key_restores_the_note_and_selected_item_immediately() {
        let (_temp, mut app) = fixture();
        let original = app.vault.read_text("0 Inbox/Learning.md").unwrap();
        app.action('a').unwrap();
        finish_save(&mut app);
        assert_eq!(app.index, 1);
        assert!(app.undo_preview.is_some());
        app.action('U').unwrap();
        assert!(app.view == View::Undo);
        assert!(render(&mut app, 130, 38).contains("Undo"));
        app.action('u').unwrap();
        finish_save(&mut app);
        assert_eq!(app.index, 0);
        assert_eq!(app.session.items[0].decision, Decision::Pending);
        assert_eq!(
            app.vault.read_text("0 Inbox/Learning.md").unwrap(),
            original
        );
        assert!(app.preview.is_some());
        assert!(app.undo_preview.is_none());
    }

    #[test]
    fn undo_key_rechecks_edits_made_after_the_screen_preview() {
        let (_temp, mut app) = fixture();
        app.action('a').unwrap();
        finish_save(&mut app);
        fs::write(
            app.vault.root.join("3 Resources/Learning.md"),
            "# Later human edit\n",
        )
        .unwrap();
        let before = app.vault.snapshot().unwrap();
        app.action('u').unwrap();
        finish_save(&mut app);
        assert!(app.error.as_ref().unwrap().contains("later edits"));
        assert_eq!(app.vault.snapshot().unwrap(), before);
        assert_eq!(app.session.items[0].decision, Decision::Applied);
    }
}
