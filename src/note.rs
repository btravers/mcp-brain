//! Ownership is checked here, independently of the MCP caller's instructions.
use anyhow::{Context, Result, bail, ensure};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::Serialize;
use sha2::{Digest, Sha256};
use yaml_serde::{Mapping, Value};

pub const AI_MARKER: &str = "<!-- ai-generated, safe to delete -->";
pub const MAX_NOTE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    Human,
    Ai,
}

pub struct Note<'a> {
    pub text: &'a str,
    pub body_start: usize,
    pub metadata: Mapping,
    pub author: Author,
}

#[derive(Debug)]
pub struct Heading {
    pub start: usize,
    pub end: usize,
    pub level: HeadingLevel,
    pub title: String,
}

pub fn revision(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Only real, top-level Markdown headings count. Examples inside code fences,
/// block quotes, or lists cannot open an editable section.
pub fn headings(text: &str) -> Vec<Heading> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current: Option<Heading> = None;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) if depth == 0 => {
                current = Some(Heading {
                    start: range.start,
                    end: range.end,
                    level,
                    title: String::new(),
                });
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::Heading(_)) if current.is_some() => {
                if let Some(h) = current.take() {
                    result.push(h);
                }
                depth -= 1;
            }
            Event::End(_) => depth -= 1,
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = current.as_mut() {
                    h.title.push_str(&t);
                }
            }
            _ => {}
        }
    }
    result
}

impl<'a> Note<'a> {
    pub fn parse(text: &'a str) -> Result<Self> {
        ensure!(text.len() <= MAX_NOTE_BYTES, "note exceeds the 2 MiB limit");
        ensure!(!text.contains('\0'), "note contains a NUL byte");
        let mut body_start = 0;
        let mut metadata = Mapping::new();
        let mut lines = text.split_inclusive('\n');
        if let Some(first) = lines
            .next()
            .filter(|l| l.trim_end_matches(['\r', '\n']) == "---")
        {
            let mut offset = first.len();
            let mut close = None;
            for line in lines {
                if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
                    close = Some((offset, offset + line.len()));
                    break;
                }
                offset += line.len();
            }
            let (yaml_end, end) =
                close.context("unclosed YAML frontmatter; repair it before editing")?;
            let yaml = &text[first.len()..yaml_end];
            if !yaml.trim().is_empty() {
                metadata =
                    yaml_serde::from_str(yaml).context("invalid or ambiguous YAML frontmatter")?;
            }
            body_start = end;
        }
        let author = match metadata.get(Value::String("author".into())) {
            Some(Value::String(s)) if s == "ai" => Author::Ai,
            _ => Author::Human,
        };
        Ok(Self {
            text,
            body_start,
            metadata,
            author,
        })
    }

    pub fn body(&self) -> &str {
        &self.text[self.body_start..]
    }

    pub fn check_revision(&self, expected: &str) -> Result<()> {
        ensure!(
            revision(self.text) == expected,
            "note changed since it was read; read it again before editing"
        );
        Ok(())
    }

    pub fn title(&self) -> Option<String> {
        headings(self.body())
            .into_iter()
            .find(|h| h.level == HeadingLevel::H1)
            .map(|h| h.title)
    }

    pub fn protected_notes_start(&self) -> Result<Option<usize>> {
        let hs = headings(self.body());
        let notes: Vec<_> = hs
            .iter()
            .filter(|h| h.level == HeadingLevel::H2 && h.title == "Notes")
            .collect();
        ensure!(
            notes.len() <= 1,
            "multiple Notes sections; refusing an ambiguous edit"
        );
        if let Some(h) = notes.first() {
            ensure!(
                !hs.iter().any(|other| other.start > h.start
                    && matches!(other.level, HeadingLevel::H1 | HeadingLevel::H2)),
                "Notes must be the trailing section; refusing to edit"
            );
            return Ok(Some(self.body_start + h.start));
        }
        Ok(None)
    }

    /// The caller supplies only the AI body. Frontmatter and the entire human
    /// suffix are carried over verbatim, including line endings and whitespace.
    pub fn replace_ai_body(&self, body: &str) -> Result<String> {
        ensure!(
            self.author == Author::Ai,
            "human notes cannot be rewritten; use comment_note or set_ai_section"
        );
        ensure!(
            !body.starts_with("---\n") && !body.starts_with("---\r\n"),
            "supply a body, not frontmatter"
        );
        ensure!(
            !headings(body)
                .iter()
                .any(|h| h.level == HeadingLevel::H2 && h.title == "Notes"),
            "Notes is reserved for the human and is preserved by the server"
        );
        let suffix = self
            .protected_notes_start()?
            .map(|i| &self.text[i..])
            .unwrap_or("");
        let result = format!(
            "{}{}\n\n{}",
            &self.text[..self.body_start],
            body.trim_end(),
            suffix
        );
        let parsed = Note::parse(&result)?;
        if !suffix.is_empty() {
            ensure!(
                parsed.protected_notes_start()? == Some(result.len() - suffix.len()),
                "replacement Markdown would hide or alter the protected Notes section"
            );
        }
        Ok(result)
    }

    fn ai_section(&self) -> Result<Option<(usize, usize)>> {
        let hs = headings(self.body());
        let ai: Vec<_> = hs
            .iter()
            .filter(|h| h.level == HeadingLevel::H2 && h.title == "AI")
            .collect();
        ensure!(
            ai.len() <= 1,
            "multiple AI sections; refusing an ambiguous edit"
        );
        let Some(h) = ai.first() else {
            return Ok(None);
        };
        ensure!(
            !hs.iter().any(|other| other.start > h.start
                && matches!(other.level, HeadingLevel::H1 | HeadingLevel::H2)),
            "the AI section must be last"
        );
        let heading_end = self.body_start + h.end;
        let rest = &self.text[heading_end..];
        let marker_line = rest.lines().next().unwrap_or("");
        ensure!(
            marker_line == AI_MARKER,
            "existing AI section is not marked as AI-generated; the human must mark it first"
        );
        let marker_end = heading_end + rest.split_inclusive('\n').next().unwrap_or("").len();
        Ok(Some((self.body_start + h.start, marker_end)))
    }

    pub fn ai_section_content(&self) -> Result<Option<&str>> {
        Ok(self.ai_section()?.map(|(_, end)| &self.text[end..]))
    }

    pub fn set_ai_section(&self, content: &str, append: bool) -> Result<String> {
        ensure!(
            self.author == Author::Human,
            "use update_ai_note for AI-authored notes"
        );
        ensure!(
            !headings(content)
                .iter()
                .any(|h| matches!(h.level, HeadingLevel::H1 | HeadingLevel::H2)),
            "AI section content may use level 3 or deeper headings only"
        );
        ensure!(
            !content.contains(AI_MARKER),
            "the AI marker is managed by the server"
        );
        let result = match self.ai_section()? {
            Some((_, end)) if !append => format!("{}{}\n", &self.text[..end], content.trim_end()),
            Some(_) => format!("{}\n{}\n", self.text, content.trim_end()),
            None => format!(
                "{}\n\n## AI\n{AI_MARKER}\n{}\n",
                self.text,
                content.trim_end()
            ),
        };
        let parsed = Note::parse(&result)?;
        ensure!(
            parsed.ai_section()?.is_some(),
            "Markdown would hide the AI section; close open code fences or HTML blocks first"
        );
        Ok(result)
    }

    /// Metadata additions never reserialize or replace existing YAML lines.
    pub fn add_up(&self, parent: &str) -> Result<String> {
        let value = format!("[[{parent}]]");
        if let Some(existing) = self.metadata.get(Value::String("up".into())) {
            if existing.as_str() != Some(&value) && self.author == Author::Ai {
                let mut metadata = self.metadata.clone();
                metadata.insert(Value::String("up".into()), Value::String(value));
                return Ok(format!(
                    "---\n{}---\n{}",
                    yaml_serde::to_string(&metadata)?,
                    self.body()
                ));
            }
            ensure!(
                existing.as_str() == Some(&value),
                "existing up field points elsewhere; the human must update it before moving"
            );
            return Ok(self.text.into());
        }
        let line = format!("up: {}\n", serde_json::to_string(&value)?);
        if self.body_start == 0 {
            Ok(format!(
                "---\nauthor: {}\n{line}---\n{}",
                if self.author == Author::Ai {
                    "ai"
                } else {
                    "human"
                },
                self.text
            ))
        } else {
            let first_end = self.text.find('\n').context("invalid frontmatter")? + 1;
            Ok(format!(
                "{}{line}{}",
                &self.text[..first_end],
                &self.text[first_end..]
            ))
        }
    }
}

pub fn new_ai_note(title: &str, body: &str) -> Result<String> {
    ensure!(
        !title.trim().is_empty() && !title.contains(['\n', '\r']),
        "title must be a nonempty single line"
    );
    if headings(body)
        .iter()
        .any(|h| h.level == HeadingLevel::H2 && h.title == "Notes")
    {
        bail!("Notes is reserved for the human; omit it from the supplied body");
    }
    let text = format!(
        "---\ncreated: {}\nauthor: ai\ntags: []\n---\n# {title}\n\n{}\n\n## Notes\n",
        chrono::Local::now().format("%Y-%m-%d"),
        body.trim_end()
    );
    ensure!(
        Note::parse(&text)?.protected_notes_start()?.is_some(),
        "Markdown would hide the protected Notes section; close open code fences or HTML blocks first"
    );
    Ok(text)
}
