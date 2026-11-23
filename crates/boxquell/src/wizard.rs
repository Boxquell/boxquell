//! `boxquell new` without a name: a form to pick agents, languages and tools.

use anyhow::{bail, Result};
use boxquell_core::recipe::{Catalog, Category};
use boxquell_core::{spec::validate_name, Backend};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub struct Choice {
    pub name: String,
    pub workspace: Option<PathBuf>,
    pub cpus: u8,
    pub ram_mib: u32,
    pub disk_gib: u32,
    pub backend: Backend,
    pub recipes: Vec<String>,
}

enum Field {
    Header(&'static str),
    Text {
        label: &'static str,
        value: String,
        digits: bool,
    },
    /// `versions` is empty for recipes without a choice; `pick` indexes into it.
    Check {
        id: String,
        label: String,
        about: String,
        on: bool,
        versions: Vec<String>,
        pick: usize,
    },
    Backend(Backend),
}

pub enum Step {
    Continue,
    Cancel,
    Done(Choice),
}

pub struct Form {
    fields: Vec<Field>,
    cursor: usize,
    error: Option<String>,
}

impl Form {
    pub fn new(catalog: &Catalog, workspace: &str) -> Self {
        let text = |label, value: &str, digits| Field::Text { label, value: value.to_string(), digits };
        let mut fields = vec![Field::Header("Sandbox"), text("Name", "", false), text("Project folder", workspace, false)];
        for (category, title) in [(Category::Agent, "Agents"), (Category::Language, "Languages"), (Category::Tool, "Tools"), (Category::Ide, "IDEs")]
        {
            let recipes: Vec<_> = catalog.all().into_iter().filter(|r| r.category == category).collect();
            if !recipes.is_empty() {
                fields.push(Field::Header(title));
            }
            for r in recipes {
                fields.push(Field::Check {
                    id: r.id.clone(),
                    label: r.name.clone(),
                    about: r.description.clone(),
                    on: false,
                    versions: r.versions.clone(),
                    pick: 0,
                });
            }
        }
        fields.extend([
            Field::Header("Machine"),
            text("CPUs", "2", true),
            text("Memory (MiB)", "4096", true),
            text("Disk (GiB)", "20", true),
            Field::Backend(Backend::default_for_host()),
        ]);
        Self { fields, cursor: 1, error: None }
    }

    fn step(&mut self, by: isize) {
        let len = self.fields.len() as isize;
        let mut at = self.cursor as isize;
        loop {
            at = (at + by).rem_euclid(len);
            if !matches!(self.fields[at as usize], Field::Header(_)) {
                break;
            }
        }
        self.cursor = at as usize;
    }

    pub fn key(&mut self, key: KeyEvent) -> Step {
        self.error = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, &mut self.fields[self.cursor]) {
            (KeyCode::Esc, _) => return Step::Cancel,
            (KeyCode::Char('c'), _) if ctrl => return Step::Cancel,
            (KeyCode::Up | KeyCode::BackTab, _) => self.step(-1),
            (KeyCode::Down | KeyCode::Tab, _) => self.step(1),
            (KeyCode::Enter, _) => match self.choice() {
                Ok(choice) => return Step::Done(choice),
                Err(err) => self.error = Some(err.to_string()),
            },
            (KeyCode::Char(' '), Field::Check { on, .. }) => *on = !*on,
            (KeyCode::Left | KeyCode::Right, Field::Check { versions, pick, on, .. }) if !versions.is_empty() => {
                let step = if key.code == KeyCode::Right { 1 } else { versions.len() - 1 };
                *pick = (*pick + step) % versions.len();
                *on = true;
            }
            (KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right, Field::Backend(backend)) => {
                *backend = if *backend == Backend::Krun { Backend::Qemu } else { Backend::Krun };
            }
            (KeyCode::Char(c), Field::Text { value, digits, .. }) if !ctrl && (!*digits || c.is_ascii_digit()) => value.push(c),
            (KeyCode::Backspace, Field::Text { value, .. }) => drop(value.pop()),
            _ => {}
        }
        Step::Continue
    }

    fn text(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find_map(|f| match f {
                Field::Text { label: l, value, .. } if *l == label => Some(value.trim()),
                _ => None,
            })
            .unwrap_or("")
    }

    fn number<T: std::str::FromStr + PartialOrd>(&self, label: &str, min: T) -> Result<T> {
        match self.text(label).parse::<T>() {
            Ok(n) if n >= min => Ok(n),
            _ => bail!("{label}: enter a number"),
        }
    }

    fn choice(&self) -> Result<Choice> {
        let name = self.text("Name").to_string();
        if name.is_empty() {
            bail!("Name: give the sandbox a name");
        }
        validate_name(&name)?;
        let workspace = match self.text("Project folder") {
            "" => None,
            dir => {
                let path = std::path::absolute(dir)?;
                if !path.is_dir() {
                    bail!("Project folder: {} is not a directory", path.display());
                }
                Some(path)
            }
        };
        Ok(Choice {
            name,
            workspace,
            cpus: self.number("CPUs", 1)?,
            ram_mib: self.number("Memory (MiB)", 256)?,
            disk_gib: self.number("Disk (GiB)", 4)?,
            backend: self.fields.iter().find_map(|f| if let Field::Backend(b) = f { Some(*b) } else { None }).unwrap_or(Backend::default_for_host()),
            recipes: self
                .fields
                .iter()
                .filter_map(|f| match f {
                    Field::Check { id, on: true, versions, pick, .. } => Some(match versions.get(*pick) {
                        Some(version) => format!("{id}@{version}"),
                        None => id.clone(),
                    }),
                    _ => None,
                })
                .collect(),
        })
    }

    /// One line per field plus the index of the line under the cursor.
    fn lines(&self) -> Vec<Line<'static>> {
        let dim = Style::new().fg(Color::DarkGray);
        self.fields
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let here = i == self.cursor;
                let mark = Span::raw(if here { "> " } else { "  " });
                let focus = if here { Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan) } else { Style::new() };
                match field {
                    Field::Header(title) => {
                        Line::from(Span::styled(format!("── {title} "), Style::new().add_modifier(Modifier::BOLD).fg(Color::Yellow)))
                    }
                    Field::Text { label, value, .. } => Line::from(vec![
                        mark,
                        Span::styled(format!("{label:<15}"), focus),
                        Span::raw(value.clone()),
                        Span::styled(if here { "▏" } else { "" }, focus),
                    ]),
                    Field::Check { label, about, on, versions, pick, .. } => Line::from(vec![
                        mark,
                        Span::styled(format!("[{}] {label:<18}", if *on { "x" } else { " " }), focus),
                        Span::raw(versions.get(*pick).map_or(String::new(), |v| format!("‹ {v:<5}› "))),
                        Span::styled(about.clone(), dim),
                    ]),
                    Field::Backend(backend) => Line::from(vec![
                        mark,
                        Span::styled(format!("{:<15}", "Backend"), focus),
                        Span::raw(if *backend == Backend::Krun { "‹ krun ›" } else { "‹ qemu ›" }),
                    ]),
                }
            })
            .collect()
    }
}

/// Show the form; `None` when the user cancels.
pub fn run(catalog: &Catalog) -> Result<Option<Choice>> {
    let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    let mut form = Form::new(catalog, &cwd);
    let mut terminal = ratatui::init();
    let outcome = (|| loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let mut lines = form.lines();
            lines.push(Line::raw(""));
            lines.push(match &form.error {
                Some(err) => Line::styled(err.clone(), Style::new().fg(Color::Red)),
                None => Line::styled("↑/↓ move   space select   ←/→ version   enter create   esc cancel", Style::new().fg(Color::DarkGray)),
            });
            // Keep the cursor line visible on short terminals.
            let visible = area.height.saturating_sub(2) as usize;
            let scroll = (form.cursor + 3).saturating_sub(visible) as u16;
            frame.render_widget(
                Paragraph::new(lines).block(Block::bordered().title(" New sandbox ")).wrap(Wrap { trim: false }).scroll((scroll, 0)),
                area,
            );
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match form.key(key) {
                Step::Continue => {}
                Step::Cancel => return Ok(None),
                Step::Done(choice) => return Ok(Some(choice)),
            }
        }
    })();
    ratatui::restore();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(form: &mut Form, code: KeyCode) -> Step {
        form.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn typing_a_name_and_ticking_an_agent_yields_a_choice() {
        let catalog = boxquell_core::recipe::fixture_catalog(&[]);
        let mut form = Form::new(&catalog, "");
        assert!(matches!(press(&mut form, KeyCode::Enter), Step::Continue), "an empty name is refused");
        assert!(form.error.is_some());
        for c in "box1".chars() {
            press(&mut form, KeyCode::Char(c));
        }
        press(&mut form, KeyCode::Down); // project folder
        press(&mut form, KeyCode::Down); // first agent; the header is skipped
        press(&mut form, KeyCode::Char(' '));
        let Step::Done(choice) = press(&mut form, KeyCode::Enter) else { panic!("form should be complete") };
        assert_eq!(choice.name, "box1");
        assert_eq!(choice.workspace, None);
        assert_eq!(choice.recipes, ["agents:claude"]);

        // Left/right on a language picks a version and selects it.
        let node = form.fields.iter().position(|f| matches!(f, Field::Check { id, .. } if id == "languages:node")).unwrap();
        form.cursor = node;
        press(&mut form, KeyCode::Right);
        let Step::Done(choice) = press(&mut form, KeyCode::Enter) else { panic!("form should be complete") };
        let second = catalog.find("node").unwrap().versions[1].clone();
        assert_eq!(choice.recipes, ["agents:claude".to_string(), format!("languages:node@{second}")]);
        assert_eq!((choice.cpus, choice.ram_mib, choice.disk_gib), (2, 4096, 20));
    }
}
