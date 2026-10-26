use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use mcp_brain::{plans::PlanListing, vault::Vault};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};

use crate::terminal::safe;

struct Picker {
    listing: PlanListing,
    filter: String,
    filtering: bool,
    selected: usize,
    list: ListState,
    list_area: Rect,
    color: bool,
    usable: bool,
}

impl Picker {
    fn indices(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        self.listing
            .plans
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                format!("{} {} {}", p.id, p.status, p.names.join(" "))
                    .to_lowercase()
                    .contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_id(&self) -> Option<String> {
        self.indices()
            .get(self.selected)
            .map(|i| self.listing.plans[*i].id.clone())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.usable = area.width >= 50 && area.height >= 18;
        if !self.usable {
            frame.render_widget(
                Paragraph::new("Enlarge to at least 50 × 18. Press q to close."),
                area,
            );
            return;
        }
        let accent = if self.color {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new("Choose a plan by its notes and progress. Enter opens the review.")
                .wrap(Wrap { trim: false })
                .style(accent)
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .title(" RECENT INBOX PLANS "),
                ),
            rows[0],
        );
        let regions = if area.width >= 110 {
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(rows[1])
        } else {
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(rows[1])
        };
        let indices = self.indices();
        self.selected = self.selected.min(indices.len().saturating_sub(1));
        self.list.select(if indices.is_empty() {
            None
        } else {
            Some(self.selected)
        });
        let entries: Vec<_> = indices
            .iter()
            .map(|i| {
                let p = &self.listing.plans[*i];
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(format!("{}  ", p.status), accent),
                        Span::raw(format!(
                            "{} · {} / {} moved",
                            p.created.format("%Y-%m-%d %H:%M"),
                            p.applied,
                            p.moves
                        )),
                    ]),
                    Line::raw(safe(&p.names.join(" · "))),
                    Line::raw(""),
                ])
            })
            .collect();
        let block = Block::bordered().title(format!(" Plans · {} shown ", indices.len()));
        self.list_area = block.inner(regions[0]);
        if entries.is_empty() {
            let message = if self.listing.plans.is_empty() {
                "No saved proposals. Ask the assistant to prepare an Inbox plan first."
            } else {
                "No plan matches this filter. Press / to change it."
            };
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: false })
                    .block(block),
                regions[0],
            );
        } else {
            frame.render_stateful_widget(
                List::new(entries)
                    .block(block)
                    .highlight_style(accent.add_modifier(Modifier::REVERSED)),
                regions[0],
                &mut self.list,
            );
            if indices.len() * 3 > self.list_area.height as usize {
                frame.render_stateful_widget(
                    Scrollbar::new(ScrollbarOrientation::VerticalRight),
                    regions[0],
                    &mut ScrollbarState::new(indices.len()).position(self.selected),
                );
            }
        }
        let details = if let Some(i) = indices.get(self.selected) {
            let p = &self.listing.plans[*i];
            format!(
                "Status: {}\n\n{} proposed · {} applied\n{} dropped · {} need precision\n\nCreated: {}\nExpires: {}\n\nNotes:\n{}\n\nPlan ID:\n{}\n\n{}",
                p.status,
                p.moves,
                p.applied,
                p.dropped,
                p.commented,
                p.created.format("%Y-%m-%d %H:%M UTC"),
                p.expires.format("%Y-%m-%d %H:%M UTC"),
                safe(&p.names.join("\n")),
                p.id,
                if p.status == "Ready" {
                    "Enter resumes this plan. It does not apply a move."
                } else {
                    "Enter opens it for inspection. Expired or stale proposals cannot apply moves."
                }
            )
        } else {
            "Your notes stay unchanged until you explicitly apply a move.".into()
        };
        frame.render_widget(
            Paragraph::new(details)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" Selected plan ")),
            regions[1],
        );
        let filter = format!("{}{}", self.filter, if self.filtering { "▏" } else { "" });
        frame.render_widget(
            Paragraph::new(safe(&filter)).block(Block::bordered().title(if self.filtering {
                " Filter · Enter done · Esc clear "
            } else {
                " Filter · / to edit "
            })),
            rows[2],
        );
        let hints = if self.listing.warnings.is_empty() && area.width >= 60 {
            "↑↓ select · Enter open · / filter · r refresh · q cancel".into()
        } else if self.listing.warnings.is_empty() {
            "↑↓ select · Enter open · q cancel\n/ filter · r refresh".into()
        } else {
            format!(
                "↑↓ select · Enter open · / filter · q cancel\n{} unreadable; inbox plans --json for details.",
                self.listing.warnings.len()
            )
        };
        frame.render_widget(Paragraph::new(hints).style(accent), rows[3]);
    }
}

pub fn run(vault: &Vault, color: bool, limit: usize) -> Result<Option<String>> {
    let mut picker = Picker {
        listing: vault.inbox_plans(limit)?,
        filter: String::new(),
        filtering: false,
        selected: 0,
        list: ListState::default(),
        list_area: Rect::default(),
        color,
        usable: true,
    };
    crate::screen::with_terminal(|terminal| {
        loop {
            terminal.draw(|frame| picker.draw(frame))?;
            let event = event::read()?;
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        if key.code == KeyCode::Char('c') {
                            return Ok(None);
                        }
                        continue;
                    }
                    if !picker.usable {
                        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                            return Ok(None);
                        }
                        continue;
                    }
                    if picker.filtering {
                        match key.code {
                            KeyCode::Esc => {
                                picker.filter.clear();
                                picker.filtering = false;
                            }
                            KeyCode::Enter => picker.filtering = false,
                            KeyCode::Backspace => {
                                picker.filter.pop();
                                picker.selected = 0;
                            }
                            KeyCode::Char(c) if picker.filter.len() + c.len_utf8() <= 400 => {
                                picker.filter.push(c);
                                picker.selected = 0;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                        KeyCode::Enter => {
                            if let Some(id) = picker.selected_id() {
                                return Ok(Some(id));
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            picker.selected = picker
                                .selected
                                .saturating_add(1)
                                .min(picker.indices().len().saturating_sub(1))
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            picker.selected = picker.selected.saturating_sub(1)
                        }
                        KeyCode::PageDown => {
                            picker.selected = picker
                                .selected
                                .saturating_add(5)
                                .min(picker.indices().len().saturating_sub(1))
                        }
                        KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(5),
                        KeyCode::Home => picker.selected = 0,
                        KeyCode::End => picker.selected = picker.indices().len().saturating_sub(1),
                        KeyCode::Char('/') => picker.filtering = true,
                        KeyCode::Char('r') => {
                            picker.listing = vault.inbox_plans(limit)?;
                            picker.selected = 0;
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) if picker.usable => match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        picker.selected = picker
                            .selected
                            .saturating_add(1)
                            .min(picker.indices().len().saturating_sub(1))
                    }
                    MouseEventKind::ScrollUp => picker.selected = picker.selected.saturating_sub(1),
                    MouseEventKind::Down(event::MouseButton::Left)
                        if picker.list_area.contains((mouse.column, mouse.row).into()) =>
                    {
                        picker.selected = (picker.list.offset()
                            + (mouse.row - picker.list_area.y) as usize / 3)
                            .min(picker.indices().len().saturating_sub(1));
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => continue,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use mcp_brain::plans::PlanSummary;
    use ratatui::{Terminal, backend::TestBackend};

    fn fixture() -> Picker {
        let now = Utc::now();
        Picker {
            listing: PlanListing {
                plans: (0..30)
                    .map(|i| PlanSummary {
                        id: format!("plan-{i}"),
                        created: now,
                        expires: now + Duration::hours(24),
                        moves: 3,
                        applied: 1,
                        commented: 1,
                        dropped: 0,
                        status: "Ready".into(),
                        names: vec![format!("Note {i}")],
                        reviewed: true,
                    })
                    .collect(),
                warnings: vec![],
            },
            filter: String::new(),
            filtering: false,
            selected: 0,
            list: ListState::default(),
            list_area: Rect::default(),
            color: false,
            usable: true,
        }
    }

    #[test]
    fn picker_formats_progress_actions_and_scrolls_selection_into_view() {
        let mut picker = fixture();
        for (width, height) in [(50, 20), (130, 38)] {
            picker.selected = 29;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| picker.draw(frame)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("Note 29") && text.contains("1 / 3 moved"),
                "{text}"
            );
            assert!(
                text.contains("Enter open") && text.contains("q cancel"),
                "{text}"
            );
            assert!(picker.list.offset() > 0);
            assert_eq!(picker.selected_id().as_deref(), Some("plan-29"));
        }
    }

    #[test]
    fn filter_matches_notes_or_status_and_safely_handles_no_matches() {
        let mut picker = fixture();
        picker.filter = "NOTE 17".into();
        assert_eq!(picker.selected_id().as_deref(), Some("plan-17"));
        picker.filter = "ready".into();
        assert_eq!(picker.indices().len(), 30);
        picker.filter = "no such proposal".into();
        assert!(picker.selected_id().is_none());
    }
}
