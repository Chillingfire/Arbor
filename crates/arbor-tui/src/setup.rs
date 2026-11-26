//! The dialogs shown before any Session can start: the API key popup when none is configured, and
//! the choice of model and temperature per role when no probe is cached for the config (grilling
//! log Round 3 decision 6).

use arbor_config::{RoleConfig, Roles};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use crate::ui::{ACCENT, BAR, MUTED, RED, YELLOW};

#[derive(Debug, Clone, PartialEq)]
pub enum SetupDialog {
    ApiKey(ApiKeyDialog),
    Models(Box<ModelsDialog>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyDialog {
    pub typed_key: String,
    /// Once a key is typed, the Operator picks where it is kept.
    pub choosing_storage: bool,
    /// Why the last key was refused or couldn't be checked.
    pub problem: Option<String>,
    pub checking: bool,
    /// The key OpenRouter couldn't be reached to check, which can be checked again or used as is.
    pub unchecked: Option<UncheckedKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncheckedKey {
    pub api_key: String,
    pub storage: KeyStorage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsDialog {
    /// The roles the Operator starts from: the current config.
    pub current: Roles,
    /// The model and temperature fields as typed, while setting them by hand.
    pub manual: Option<ManualRoles>,
    /// Why the last choice failed its range check or probe.
    pub problems: Vec<String>,
    pub checking: bool,
    /// Set when OpenRouter couldn't be reached to probe `current`, which can be probed again or
    /// used unprobed.
    pub unreachable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualRoles {
    pub rows: [ManualRole; 3],
    /// The field being typed into: two per row, model first.
    pub selected_field: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualRole {
    pub role: &'static str,
    pub model: String,
    pub temperature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStorage {
    UserConfig,
    SessionOnly,
}

/// What the Operator settled in a setup dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum SetupOutcome {
    Quit,
    ApiKey {
        api_key: String,
        storage: KeyStorage,
    },
    Roles(Roles),
    /// The key, used for this run without a check because OpenRouter couldn't be reached.
    UncheckedApiKey(String),
    /// The roles, used without a probe because OpenRouter couldn't be reached.
    UnprobedRoles(Roles),
}

const FIELD_COUNT: usize = 6;

impl SetupDialog {
    pub fn api_key(problem: Option<String>) -> Self {
        Self::ApiKey(ApiKeyDialog {
            typed_key: String::new(),
            choosing_storage: false,
            problem,
            checking: false,
            unchecked: None,
        })
    }

    /// The key dialog after OpenRouter couldn't be reached to check `api_key`.
    pub fn unchecked_api_key(problem: String, api_key: String, storage: KeyStorage) -> Self {
        Self::ApiKey(ApiKeyDialog {
            typed_key: api_key.clone(),
            choosing_storage: false,
            problem: Some(problem),
            checking: false,
            unchecked: Some(UncheckedKey { api_key, storage }),
        })
    }

    pub fn models(current: Roles, problems: Vec<String>) -> Self {
        Self::Models(Box::new(ModelsDialog {
            current,
            manual: None,
            problems,
            checking: false,
            unreachable: false,
        }))
    }

    /// The models dialog after OpenRouter couldn't be reached to probe `chosen`.
    pub fn unprobed_models(chosen: Roles, problems: Vec<String>) -> Self {
        Self::Models(Box::new(ModelsDialog {
            current: chosen,
            manual: None,
            problems,
            checking: false,
            unreachable: true,
        }))
    }

    /// While a check runs, keys are ignored and the dialog says what it is waiting on.
    pub fn set_checking(&mut self, checking: bool) {
        match self {
            Self::ApiKey(dialog) => dialog.checking = checking,
            Self::Models(dialog) => dialog.checking = checking,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SetupOutcome> {
        match self {
            Self::ApiKey(dialog) if !dialog.checking => dialog.handle_key(key),
            Self::Models(dialog) if !dialog.checking => dialog.handle_key(key),
            _ => None,
        }
    }

    pub fn paste(&mut self, pasted: &str) {
        let single_line = pasted.trim();
        match self {
            Self::ApiKey(dialog)
                if !dialog.checking && !dialog.choosing_storage && dialog.unchecked.is_none() =>
            {
                dialog.typed_key.push_str(single_line);
            }
            Self::Models(dialog) if !dialog.checking => {
                if let Some(manual) = &mut dialog.manual {
                    manual.selected_text().push_str(single_line);
                }
            }
            _ => {}
        }
    }
}

impl ApiKeyDialog {
    fn handle_key(&mut self, key: KeyEvent) -> Option<SetupOutcome> {
        if let Some(unchecked) = &self.unchecked {
            return match key.code {
                KeyCode::Enter => Some(SetupOutcome::ApiKey {
                    api_key: unchecked.api_key.clone(),
                    storage: unchecked.storage,
                }),
                KeyCode::Char('c') => {
                    Some(SetupOutcome::UncheckedApiKey(unchecked.api_key.clone()))
                }
                KeyCode::Esc => Some(SetupOutcome::Quit),
                _ => None,
            };
        }
        if self.choosing_storage {
            let storage = match key.code {
                KeyCode::Char('s') => KeyStorage::UserConfig,
                KeyCode::Char('o') => KeyStorage::SessionOnly,
                KeyCode::Esc => {
                    self.choosing_storage = false;
                    return None;
                }
                _ => return None,
            };
            return Some(SetupOutcome::ApiKey {
                api_key: self.typed_key.trim().to_owned(),
                storage,
            });
        }
        match key.code {
            KeyCode::Esc => return Some(SetupOutcome::Quit),
            KeyCode::Enter if !self.typed_key.trim().is_empty() => self.choosing_storage = true,
            KeyCode::Backspace => {
                self.typed_key.pop();
            }
            KeyCode::Char(typed_char) => self.typed_key.push(typed_char),
            _ => {}
        }
        None
    }
}

impl ModelsDialog {
    fn handle_key(&mut self, key: KeyEvent) -> Option<SetupOutcome> {
        let Some(manual) = &mut self.manual else {
            return match key.code {
                KeyCode::Enter if self.unreachable => {
                    Some(SetupOutcome::Roles(self.current.clone()))
                }
                KeyCode::Char('c') if self.unreachable => {
                    Some(SetupOutcome::UnprobedRoles(self.current.clone()))
                }
                KeyCode::Char('r') => Some(SetupOutcome::Roles(Roles::recommended())),
                KeyCode::Char('m') | KeyCode::Enter => {
                    self.manual = Some(ManualRoles::from(&self.current));
                    None
                }
                KeyCode::Esc => Some(SetupOutcome::Quit),
                _ => None,
            };
        };
        match key.code {
            KeyCode::Esc => self.manual = None,
            KeyCode::Down | KeyCode::Tab => {
                manual.selected_field = (manual.selected_field + 1) % FIELD_COUNT;
            }
            KeyCode::Up | KeyCode::BackTab => {
                manual.selected_field = (manual.selected_field + FIELD_COUNT - 1) % FIELD_COUNT;
            }
            KeyCode::Backspace => {
                manual.selected_text().pop();
            }
            KeyCode::Char(typed_char) => manual.selected_text().push(typed_char),
            KeyCode::Enter => match manual.roles() {
                Ok(roles) => return Some(SetupOutcome::Roles(roles)),
                Err(problems) => self.problems = problems,
            },
            _ => {}
        }
        None
    }
}

impl From<&Roles> for ManualRoles {
    fn from(roles: &Roles) -> Self {
        let row = |(role, role_config): (&'static str, &RoleConfig)| ManualRole {
            role,
            model: role_config.model.clone(),
            temperature: role_config.temperature.to_string(),
        };
        Self {
            rows: roles.named().map(row),
            selected_field: 0,
        }
    }
}

impl ManualRoles {
    fn selected_text(&mut self) -> &mut String {
        let row = &mut self.rows[self.selected_field / 2];
        if self.selected_field.is_multiple_of(2) {
            &mut row.model
        } else {
            &mut row.temperature
        }
    }

    /// The typed roles, or every problem with them.
    pub fn roles(&self) -> Result<Roles, Vec<String>> {
        let mut problems = Vec::new();
        let role_configs = self.rows.clone().map(|row| {
            let model = row.model.trim().to_owned();
            if model.is_empty() {
                problems.push(format!("the {} needs a model", row.role));
            }
            let temperature = row.temperature.trim().parse::<f64>().unwrap_or_else(|_| {
                problems.push(format!(
                    "the {} temperature {:?} is not a number",
                    row.role, row.temperature
                ));
                0.0
            });
            let role_config = RoleConfig { model, temperature };
            if let Err(out_of_range) = role_config.check_temperature(row.role) {
                problems.push(out_of_range.to_string());
            }
            role_config
        });
        let [planner, scaffolder, writer] = role_configs;
        if problems.is_empty() {
            Ok(Roles {
                planner,
                scaffolder,
                writer,
            })
        } else {
            Err(problems)
        }
    }
}

pub fn draw_setup(frame: &mut Frame, dialog: &SetupDialog) {
    let (title, lines, hints) = match dialog {
        SetupDialog::ApiKey(key_dialog) => (
            "OpenRouter API key",
            api_key_lines(key_dialog),
            api_key_hints(key_dialog),
        ),
        SetupDialog::Models(models_dialog) => (
            "Models and temperatures",
            models_lines(models_dialog),
            models_hints(models_dialog),
        ),
    };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(90);
    let wanted_height = u16::try_from(lines.len() + 4).unwrap_or(u16::MAX);
    let height = wanted_height.min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let mut hint_spans = vec![Span::raw(" ")];
    for (keys, action) in hints {
        hint_spans.push(Span::styled(format!("{keys} "), Style::new().fg(ACCENT)));
        hint_spans.push(Span::styled(format!("{action}  "), Style::new().fg(MUTED)));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(YELLOW))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(YELLOW).bold(),
        ))
        .title_bottom(Line::from(hint_spans).alignment(Alignment::Right))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BAR));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn api_key_lines(dialog: &ApiKeyDialog) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw("Arbor calls models through OpenRouter and has no API key yet."),
        Line::default(),
        Line::from(vec![
            Span::styled("Key  ", Style::new().fg(MUTED)),
            Span::styled(
                format!("{}▏", masked(&dialog.typed_key)),
                Style::new().fg(ACCENT),
            ),
        ]),
    ];
    if let Some(problem) = &dialog.problem {
        lines.push(Line::default());
        lines.push(Line::styled(problem.clone(), Style::new().fg(RED)));
    }
    lines.push(Line::default());
    lines.push(if dialog.checking {
        Line::styled("Checking the key with OpenRouter…", Style::new().fg(YELLOW))
    } else if dialog.unchecked.is_some() {
        Line::raw(
            "OpenRouter couldn't be reached to check it. Try again, or use it unchecked for this \
             session only.",
        )
    } else if dialog.choosing_storage {
        Line::raw("Save it to the user config, or use it for this session only?")
    } else {
        Line::styled(
            "It is checked with a free call before it is used.",
            Style::new().fg(MUTED),
        )
    });
    lines
}

/// Shows only the key's last four characters.
fn masked(typed_key: &str) -> String {
    let char_count = typed_key.chars().count();
    let shown: String = typed_key
        .chars()
        .skip(char_count.saturating_sub(4))
        .collect();
    format!("{}{shown}", "•".repeat(char_count.saturating_sub(4)))
}

fn api_key_hints(dialog: &ApiKeyDialog) -> &'static [(&'static str, &'static str)] {
    if dialog.checking {
        &[]
    } else if dialog.unchecked.is_some() {
        &[
            ("Enter", "try again"),
            ("c", "use unchecked"),
            ("Esc", "quit"),
        ]
    } else if dialog.choosing_storage {
        &[("s", "save"), ("o", "this session only"), ("Esc", "back")]
    } else {
        &[("Enter", "next"), ("Esc", "quit")]
    }
}

fn models_lines(dialog: &ModelsDialog) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw("No model check is on record for this config. Each model is probed once with"),
        Line::raw("a short call at its temperature before a Session uses it."),
        Line::default(),
    ];
    match &dialog.manual {
        Some(manual) => lines.extend(manual_lines(manual)),
        None => {
            lines.push(heading_line(if dialog.unreachable {
                "Chosen"
            } else {
                "Current"
            }));
            lines.extend(role_lines(&dialog.current));
            lines.push(Line::default());
            lines.push(heading_line("Recommended"));
            lines.extend(role_lines(&Roles::recommended()));
        }
    }
    for problem in &dialog.problems {
        lines.push(Line::default());
        lines.push(Line::styled(problem.clone(), Style::new().fg(RED)));
    }
    if dialog.unreachable && dialog.manual.is_none() {
        lines.push(Line::default());
        lines.push(Line::raw(
            "OpenRouter couldn't be reached to probe them. Try again, or continue without the \
             probe: they are probed again next run.",
        ));
    }
    if dialog.checking {
        lines.push(Line::default());
        lines.push(Line::styled("Probing the models…", Style::new().fg(YELLOW)));
    }
    lines
}

fn heading_line(heading: &str) -> Line<'static> {
    Line::styled(heading.to_owned(), Style::new().fg(MUTED).bold())
}

fn role_lines(roles: &Roles) -> Vec<Line<'static>> {
    roles
        .named()
        .into_iter()
        .map(|(role, role_config)| {
            Line::raw(format!(
                "  {role:<11}{}  at {}",
                role_config.model, role_config.temperature
            ))
        })
        .collect()
}

fn manual_lines(manual: &ManualRoles) -> Vec<Line<'static>> {
    let field = |typed: &str, field_index: usize| {
        if field_index == manual.selected_field {
            Span::styled(
                format!("{typed}▏"),
                Style::new().fg(ACCENT).add_modifier(Modifier::UNDERLINED),
            )
        } else {
            Span::raw(typed.to_owned())
        }
    };
    manual
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            Line::from(vec![
                Span::styled(format!("  {:<11}", row.role), Style::new().fg(MUTED)),
                field(&row.model, row_index * 2),
                Span::styled("  temperature ", Style::new().fg(MUTED)),
                field(&row.temperature, row_index * 2 + 1),
            ])
        })
        .collect()
}

fn models_hints(dialog: &ModelsDialog) -> &'static [(&'static str, &'static str)] {
    if dialog.checking {
        &[]
    } else if dialog.manual.is_some() {
        &[("↑↓", "field"), ("Enter", "probe"), ("Esc", "back")]
    } else if dialog.unreachable {
        &[
            ("Enter", "try again"),
            ("c", "continue unprobed"),
            ("m", "set by hand"),
            ("r", "use recommended"),
            ("Esc", "quit"),
        ]
    } else {
        &[
            ("m", "set by hand"),
            ("r", "use recommended"),
            ("Esc", "quit"),
        ]
    }
}
