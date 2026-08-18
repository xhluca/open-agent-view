use std::time::{Duration, SystemTime};

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Frame, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{
    is_active_session_state, project_group_path, App, ComposerMode, ConfirmTarget, Overlay,
    SelectionKey, ViewMode,
};
use crate::domain::{AgentSession, Capability, Provider, SessionState};

const BG: Color = Color::Rgb(24, 26, 27);
const FG: Color = Color::Rgb(205, 205, 205);
const DIM: Color = Color::Rgb(145, 145, 145);
const SELECTED_BG: Color = Color::Rgb(58, 60, 61);
const ACCENT: Color = Color::Rgb(89, 194, 201);
const ATTENTION: Color = Color::Rgb(232, 191, 72);
const COMPLETE: Color = Color::Rgb(101, 187, 120);

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.size();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 32 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("coding-agents needs at least 32×8")
                .style(Style::default().bg(BG).fg(ATTENTION))
                .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let composer_height = match app.overlay {
        Overlay::Peek => 7.min(area.height.saturating_sub(5)),
        Overlay::Help => {
            let help_lines = pack_help_actions(
                help_actions(app),
                area.width.saturating_sub(2) as usize,
            )
            .len() as u16;
            (3 + help_lines).min(area.height.saturating_sub(5))
        }
        _ => 3,
    };
    let header_height = if area.height < 16 { 2 } else { 4 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, chunks[0]);
    render_session_list(frame, app, chunks[1]);
    render_bottom_panel(frame, app, chunks[2]);
    render_footer(frame, app, chunks[3]);

    match &app.overlay {
        Overlay::Details => render_details(frame, app, area),
        Overlay::Confirm(target) => render_confirmation(frame, app, target, area),
        _ => {}
    }
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let cwd = header_directory(app);
    let completed = app.snapshot.count(SessionState::Completed);
    let working = app.snapshot.count(SessionState::ReadyForReview)
        + app.snapshot.count(SessionState::Working);
    let awaiting = app.snapshot.count(SessionState::NeedsInput);
    let providers = provider_summary(app);
    let title = format!("Open Agent View v{}", env!("CARGO_PKG_VERSION"));
    let mode = match app.view_mode {
        ViewMode::Status => "status",
        ViewMode::Directory => "directory",
    };

    let lines = if area.height <= 2 {
        vec![
            Line::from(vec![
                Span::styled("◇ ", Style::default().fg(ACCENT)),
                Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(format!("{awaiting} awaiting · {working} working · {completed} completed")),
        ]
    } else if area.width >= 70 {
        vec![
            Line::from(vec![
                Span::styled("  ◇◇  ", Style::default().fg(ACCENT)),
                Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled(" ◇  ◇ ", Style::default().fg(ACCENT)),
                Span::styled(format!("{providers} · {cwd}"), Style::default().fg(DIM)),
            ]),
            Line::from(vec![
                Span::styled("  ◇◇  ", Style::default().fg(ACCENT)),
                Span::styled(
                    format!(
                        "{awaiting} awaiting input · {working} working · {completed} completed · {mode} view"
                    ),
                    Style::default().fg(DIM),
                ),
            ]),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                title,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!("{providers} · {cwd}"),
                Style::default().fg(DIM),
            )),
            Line::from(Span::styled(
                format!("{awaiting} awaiting · {working} working · {completed} completed"),
                Style::default().fg(DIM),
            )),
        ]
    };
    frame.render_widget(Paragraph::new(lines).style(Style::default().bg(BG).fg(FG)), area);
}

fn render_session_list(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut lines = Vec::new();
    let mut selected_line = None;
    let selection_visible = !matches!(app.overlay, Overlay::Composer(_));

    for (group_position, group) in app.groups().into_iter().enumerate() {
        if group_position > 0 {
            lines.push(Line::default());
        }
        let is_selected = selection_visible
            && app.selection == Some(SelectionKey::Group(group.key.clone()));
        if is_selected {
            selected_line = Some(lines.len());
        }
        let collapsed = app.collapsed.contains(&group.key);
        let suffix = collapsed.then(|| format!(" {}", group.sessions.len()));
        lines.push(styled_line(
            vec![Span::styled(
                format!("{}{}", group.label, suffix.unwrap_or_default()),
                Style::default().add_modifier(Modifier::BOLD),
            )],
            is_selected,
        ));

        if collapsed {
            continue;
        }
        for index in group.sessions {
            let session = &app.snapshot.sessions[index];
            let is_selected = selection_visible
                && app.selection == Some(SelectionKey::Session(session.id.clone()));
            if is_selected {
                selected_line = Some(lines.len());
            }
            lines.push(render_session_row(session, app.view_mode, area.width, is_selected));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.filter.is_empty() {
                "No coding-agent sessions found"
            } else {
                "No sessions match the current filter"
            },
            Style::default().fg(DIM),
        )));
    }

    let scroll = selected_line
        .map(|line| line.saturating_sub(area.height.saturating_sub(1) as usize))
        .unwrap_or(0);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(BG).fg(FG))
            .scroll((scroll as u16, 0)),
        area,
    );
}

fn render_session_row(
    session: &AgentSession,
    view_mode: ViewMode,
    width: u16,
    selected: bool,
) -> Line<'static> {
    let symbol = state_symbol(session.state);
    let symbol_style = Style::default().fg(state_color(session.state));
    let name_width = if width >= 100 { 26 } else { 20 };
    let provider = match session.provider {
        Provider::Claude => "C",
        Provider::Codex => "X",
        Provider::Other(_) => "?",
    };
    let runtime = session.runtime.label();
    let state_prefix = (view_mode == ViewMode::Directory)
        .then(|| format!("{} · ", short_state(session.state)))
        .unwrap_or_default();
    let age = format_age(session.age(SystemTime::now()));
    let prs = session
        .pull_requests
        .map(|count| format!("{count} PR{}", if count == 1 { "" } else { "s" }))
        .unwrap_or_default();
    let metadata = if width >= 90 {
        format!("[{provider}@{runtime}]")
    } else {
        format!("[{provider}]")
    };
    let right = if prs.is_empty() {
        age
    } else {
        format!("{prs:>7} {age:>5}")
    };
    let fixed = 4 + name_width + metadata.len() + right.len();
    let summary_width = (width as usize).saturating_sub(fixed).max(1);
    let summary = truncate(
        &format!("{state_prefix}{}", session.summary),
        summary_width,
    );
    let spans = vec![
        Span::styled(format!(" {symbol} "), symbol_style),
        Span::styled(
            format!("{:name_width$}", truncate(&session.name, name_width)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{metadata} "), Style::default().fg(DIM)),
        Span::styled(format!("{summary:summary_width$}"), Style::default().fg(DIM)),
        Span::styled(right, Style::default().fg(DIM)),
    ];
    styled_line(spans, selected)
}

fn render_bottom_panel(frame: &mut Frame<'_>, app: &App, area: Rect) {
    match &app.overlay {
        Overlay::Peek => render_peek(frame, app, area),
        Overlay::Help => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3.min(area.height)), Constraint::Min(0)])
                .split(area);
            render_composer(frame, app, chunks[0]);
            render_help(frame, app, chunks[1]);
        }
        _ => render_composer(frame, app, area),
    }
}

fn render_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(BG));
    let (prefix, content, editable) = match &app.overlay {
        Overlay::Composer(ComposerMode::NewSession) => ("❯ ", app.input.as_str(), true),
        Overlay::Composer(ComposerMode::Reply { .. }) => ("❯ reply ", app.input.as_str(), true),
        Overlay::Composer(ComposerMode::Rename { .. }) => ("❯ name ", app.input.as_str(), true),
        Overlay::Composer(ComposerMode::Filter) => ("❯ filter ", app.input.as_str(), true),
        _ => (
            "❯ ",
            if app.filter.is_empty() {
                "describe a task for a new session"
            } else {
                "type to start a new session · / to change filter"
            },
            false,
        ),
    };
    let text_style = if editable {
        Style::default().fg(FG)
    } else {
        Style::default().fg(DIM)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix, Style::default().fg(FG)),
            Span::styled(content.to_owned(), text_style),
        ]))
        .block(block),
        area,
    );
    if editable {
        let cursor_x = area.x + 1 + prefix.chars().count() as u16 + app.input.chars().count() as u16;
        frame.set_cursor(cursor_x.min(area.right().saturating_sub(1)), area.y + 1);
    }
}

fn render_peek(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(session) = app.selected_session() else {
        render_composer(frame, app, area);
        return;
    };
    let block = Block::default()
        .title(format!(" {} · {} ", session.name, session.provider))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG));
    let summary = app.selected_detail().unwrap_or_else(|| {
        if session.summary.is_empty() {
            "No summary is available from this provider."
        } else {
            &session.summary
        }
    });
    let reply = if app.input.is_empty() {
        Span::styled("❯ reply", Style::default().fg(DIM))
    } else {
        Span::styled(format!("❯ {}", app.input), Style::default().fg(FG))
    };
    let summary_capacity = area.height.saturating_sub(4).max(1) as usize;
    let summary_lines: Vec<_> = summary.lines().collect();
    let summary_start = summary_lines.len().saturating_sub(summary_capacity);
    let mut lines: Vec<Line<'_>> = summary_lines[summary_start..]
        .iter()
        .map(|line| Line::from((*line).to_owned()))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(reply));
    frame.render_widget(
        Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true }),
        area,
    );
    if !app.input.is_empty() {
        frame.set_cursor(
            (area.x + 3 + app.input.chars().count() as u16).min(area.right().saturating_sub(2)),
            area.bottom().saturating_sub(2),
        );
    }
}

fn render_help(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lines = pack_help_actions(help_actions(app), area.width.saturating_sub(2) as usize)
        .into_iter()
        .map(|line| Line::from(format!("  {line}")))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(BG).fg(DIM)),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let footer = if let Some(notice) = &app.notice {
        notice.clone()
    } else if let Some(warning) = app.snapshot.warnings.first() {
        format!("warning: {warning}")
    } else {
        contextual_footer(app, area.width)
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().bg(BG).fg(DIM))
            .alignment(Alignment::Left),
        area,
    );
}

fn contextual_footer(app: &App, width: u16) -> String {
    match &app.overlay {
        Overlay::Composer(ComposerMode::Filter) => "enter to apply · esc to cancel".into(),
        Overlay::Composer(ComposerMode::Rename { .. }) => "enter to save · esc to cancel".into(),
        Overlay::Composer(_) if width >= 70 => {
            "enter to create · ctrl+j for newline · esc to clear".into()
        }
        Overlay::Composer(_) => "enter to create · esc to clear".into(),
        Overlay::Peek if app.input.is_empty() && width >= 80 => format!(
            "enter to open · space to close{}",
            app.selected_session()
                .map(session_control_suffix)
                .unwrap_or_default()
        ),
        Overlay::Peek if app.input.is_empty() && width >= 55 => {
            "enter to open · space to close".into()
        }
        Overlay::Peek if app.input.is_empty() => "enter to open".into(),
        Overlay::Peek => "enter to send · esc to close".into(),
        Overlay::Help => String::new(),
        Overlay::None if matches!(app.selection, Some(SelectionKey::Group(_))) => {
            let action = if selected_group_can_delete(app) {
                " · ctrl+x to delete all"
            } else {
                ""
            };
            let verb = app
                .selected_group()
                .map(|group| {
                    if app.collapsed.contains(&group.key) {
                        "expand"
                    } else {
                        "collapse"
                    }
                })
                .unwrap_or("collapse/expand");
            if width >= 80 {
                format!("enter to {verb}{action} · ? for shortcuts")
            } else {
                format!("enter to {verb} · ? for shortcuts")
            }
        }
        Overlay::None if app.selected_session().is_some() && width >= 80 => {
            let session = app.selected_session().expect("selection checked");
            let peek = session_peek_suffix(session);
            let control = session_control_suffix(session);
            format!("enter to open{peek}{control} · ? for shortcuts")
        }
        Overlay::None if app.selected_session().is_some() && width >= 55 => {
            let session = app.selected_session().expect("selection checked");
            format!(
                "enter to open{} · ? for shortcuts",
                session_peek_suffix(session)
            )
        }
        Overlay::None if app.selected_session().is_some() => {
            "enter to open · ? for shortcuts".into()
        }
        _ if width >= 70 => {
            "type to create · ↑/↓ to select · / to filter · ? for shortcuts".into()
        }
        _ => "↑/↓ to select · ? for shortcuts".into(),
    }
}

fn selected_group_can_delete(app: &App) -> bool {
    app.selected_group().is_some_and(|group| {
        group.sessions.iter().all(|index| {
            let session = &app.snapshot.sessions[*index];
            !is_active_session_state(session.state)
                && session.capabilities.contains(&Capability::Delete)
        })
    })
}

fn session_control_suffix(session: &AgentSession) -> &'static str {
    if is_active_session_state(session.state) {
        if session.capabilities.contains(&Capability::Interrupt) {
            " · ctrl+x to stop"
        } else {
            " · observe-only"
        }
    } else if session.capabilities.contains(&Capability::Delete)
        && session.capabilities.contains(&Capability::Archive)
    {
        " · ctrl+a archive · ctrl+x delete"
    } else if session.capabilities.contains(&Capability::Delete) {
        " · ctrl+x delete"
    } else {
        " · observe-only"
    }
}

fn session_peek_suffix(session: &AgentSession) -> &'static str {
    if session.capabilities.contains(&Capability::Reply) {
        " · space to reply"
    } else if session.capabilities.contains(&Capability::Inspect) {
        " · space to inspect"
    } else {
        ""
    }
}

fn help_actions(app: &App) -> Vec<String> {
    let mut actions = Vec::new();
    if app.selected_session().is_some() {
        actions.push("ctrl+r to rename".into());
    }
    actions.push("ctrl+s to switch views".into());
    actions.push("ctrl+j for newline".into());
    actions.push("/ to filter".into());
    actions.push("tab for new task".into());
    if let Some(session) = app.selected_session() {
        if session.capabilities.contains(&Capability::Reply) {
            actions.push("space to inspect/reply".into());
        } else if session.capabilities.contains(&Capability::Inspect) {
            actions.push("space to inspect".into());
        }
        let action = if is_active_session_state(session.state)
            && session.capabilities.contains(&Capability::Interrupt)
        {
            Some("ctrl+x to stop")
        } else if !is_active_session_state(session.state)
            && session.capabilities.contains(&Capability::Delete)
        {
            Some("ctrl+x to delete")
        } else {
            None
        };
        if let Some(action) = action {
            actions.push(action.into());
        }
        if !is_active_session_state(session.state)
            && session.capabilities.contains(&Capability::Archive)
        {
            actions.push("ctrl+a to archive".into());
        }
    } else if selected_group_can_delete(app) {
        actions.push("ctrl+x to delete all".into());
    }
    actions.push("esc to quit".into());
    actions.push("? to close".into());
    actions
}

fn pack_help_actions(actions: Vec<String>, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for action in actions {
        let line = lines.last_mut().expect("help always has one line");
        let added = if line.is_empty() {
            action.len()
        } else {
            4 + action.len()
        };
        if !line.is_empty() && line.len() + added > width {
            lines.push(action);
        } else {
            if !line.is_empty() {
                line.push_str("    ");
            }
            line.push_str(&action);
        }
    }
    lines
}

fn render_details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(session) = app.selected_session() else {
        return;
    };
    let popup = centered_rect(86, 80, area);
    frame.render_widget(Clear, popup);
    let capabilities = session
        .capabilities
        .iter()
        .map(|capability| format!("{capability:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let lines = vec![
        Line::from(vec![
            Span::styled(&session.name, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  {} @ {}", session.provider, session.runtime.label()),
                Style::default().fg(DIM),
            ),
        ]),
        Line::default(),
        Line::from(format!("state: {:?}", session.state)),
        Line::from(format!("session: {}", session.provider_session_id)),
        Line::from(format!("directory: {}", session.cwd.display())),
        Line::from(format!("pid: {}", session.pid.map_or("—".into(), |pid| pid.to_string()))),
        Line::from(format!("capabilities: {capabilities}")),
        Line::default(),
        Line::from(if session.summary.is_empty() {
            "No provider summary is available.".into()
        } else {
            session.summary.clone()
        }),
        Line::default(),
        Line::from(Span::styled("esc to return", Style::default().fg(DIM))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" session details ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT)),
            )
            .style(Style::default().bg(BG).fg(FG))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_confirmation(frame: &mut Frame<'_>, _: &App, target: &ConfirmTarget, area: Rect) {
    let popup = centered_rect(72, 30, area);
    frame.render_widget(Clear, popup);
    let message = match target {
        ConfirmTarget::Session { id, running: true } => {
            format!("Interrupt the exact running session?\n\n{id}\n\nEnter confirms; escape keeps it.")
        }
        ConfirmTarget::Session { id, running: false } => {
            format!("Delete the exact session record?\n\n{id}\n\nEnter confirms; escape keeps it.")
        }
        ConfirmTarget::Archive { id } => {
            format!("Archive the exact session?\n\n{id}\n\nEnter confirms; escape keeps it.")
        }
        ConfirmTarget::Group { key, session_ids } => format!(
            "Delete all {} sessions in {key}?\n\nEnter confirms; escape keeps them.",
            session_ids.len()
        ),
    };
    frame.render_widget(
        Paragraph::new(message)
            .block(
                Block::default()
                    .title(" confirm action ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ATTENTION)),
            )
            .style(Style::default().bg(BG).fg(FG))
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn provider_summary(app: &App) -> String {
    let mut labels = app
        .snapshot
        .sessions
        .iter()
        .map(|session| session.provider.label())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    if labels.is_empty() {
        "Claude + Codex".into()
    } else {
        labels.join(" + ")
    }
}

fn styled_line(spans: Vec<Span<'static>>, selected: bool) -> Line<'static> {
    let style = if selected {
        Style::default().bg(SELECTED_BG).fg(Color::White)
    } else {
        Style::default().bg(BG).fg(FG)
    };
    Line::from(spans).style(style)
}

fn state_symbol(state: SessionState) -> &'static str {
    match state {
        SessionState::ReadyForReview => "✱",
        SessionState::NeedsInput => "✱",
        SessionState::Working => "✳",
        SessionState::Completed => "•",
        SessionState::Unknown => "?",
    }
}

fn state_color(state: SessionState) -> Color {
    match state {
        SessionState::ReadyForReview | SessionState::NeedsInput => ATTENTION,
        SessionState::Working => ACCENT,
        SessionState::Completed => COMPLETE,
        SessionState::Unknown => DIM,
    }
}

fn short_state(state: SessionState) -> &'static str {
    match state {
        SessionState::ReadyForReview => "Review",
        SessionState::NeedsInput => "Needs input",
        SessionState::Working => "Working",
        SessionState::Completed => "Done",
        SessionState::Unknown => "Unknown",
    }
}

fn format_age(age: Option<Duration>) -> String {
    let Some(age) = age else {
        return "—".into();
    };
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn truncate(input: &str, width: usize) -> String {
    if input.chars().count() <= width {
        return input.into();
    }
    if width <= 1 {
        return "…".into();
    }
    let mut output: String = input.chars().take(width - 1).collect();
    output.push('…');
    output
}

fn header_directory(app: &App) -> String {
    let selected_directory = if app.view_mode == ViewMode::Directory {
        app.selected_session()
            .map(|session| project_group_path(&session.cwd))
            .or_else(|| {
                app.selected_group().and_then(|group| {
                    group
                        .sessions
                        .first()
                        .map(|index| project_group_path(&app.snapshot.sessions[*index].cwd))
                })
            })
    } else {
        None
    };
    selected_directory
        .or_else(|| std::env::current_dir().ok())
        .map(|path| abbreviate_path(&path))
        .unwrap_or_else(|| "unknown directory".into())
}

fn abbreviate_path(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .and_then(|home| path.strip_prefix(home).ok().map(std::path::PathBuf::from))
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|| path.display().to_string())
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::domain::{
        AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot,
    };

    use super::*;

    #[test]
    fn dashboard_renders_reference_sections_and_composer() {
        let snapshot = SessionSnapshot {
            sessions: vec![
                session("review", SessionState::ReadyForReview),
                session("blocked", SessionState::NeedsInput),
                session("worker", SessionState::Working),
                session("done", SessionState::Completed),
            ],
            warnings: vec![],
        };
        let app = App::new(snapshot);
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Open Agent View"));
        assert!(rendered.contains("Ready for review"));
        assert!(rendered.contains("Needs input"));
        assert!(rendered.contains("Working"));
        assert!(rendered.contains("Completed"));
        assert!(rendered.contains("describe a task for a new session"));
        assert!(rendered.contains("1 awaiting input · 2 working · 1 completed"));
    }

    #[test]
    fn tiny_terminals_render_a_clear_minimum_size_message() {
        let app = App::new(SessionSnapshot::default());
        let backend = TestBackend::new(31, 7);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();

        assert!(buffer_text(terminal.backend().buffer()).contains("needs at least"));
    }

    #[test]
    fn narrow_footer_keeps_the_help_affordance() {
        let app = App::new(SessionSnapshot {
            sessions: vec![session("worker", SessionState::Working)],
            warnings: vec![],
        });
        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("enter to open · ? for shortcuts"));
        assert!(!rendered.contains("space to reply"));
    }

    #[test]
    fn help_is_contextual_and_does_not_advertise_ungranted_control() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("worker", SessionState::Working)],
            warnings: vec![],
        });
        app.toggle_help();
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("ctrl+r to rename"));
        assert!(rendered.contains("ctrl+s to switch views"));
        assert!(rendered.contains("describe a task for a new session"));
        assert!(!rendered.contains("ctrl+x to stop"));
        assert!(!rendered.contains("j/k"));
    }

    #[test]
    fn completed_control_is_labeled_delete_not_stop() {
        let mut item = session("done", SessionState::Completed);
        item.capabilities.insert(Capability::Delete);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_help();
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("ctrl+x to delete"));
        assert!(!rendered.contains("ctrl+x to stop"));
    }

    #[test]
    fn directory_view_header_tracks_the_selected_project() {
        let mut item = session("worker", SessionState::Working);
        item.cwd = PathBuf::from("/different/project/.claude/worktrees/topic");
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_view();
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Claude · /different/project"));
        assert!(rendered.contains("/different/project"));
        assert!(!rendered.contains(".claude/worktrees"));
    }

    fn session(name: &str, state: SessionState) -> AgentSession {
        AgentSession {
            id: name.into(),
            provider_session_id: name.into(),
            provider: Provider::Claude,
            runtime: Runtime::Host,
            kind: SessionKind::Background,
            name: name.into(),
            cwd: PathBuf::from("/work"),
            state,
            summary: format!("latest summary from {name}"),
            raw_state: None,
            pid: None,
            started_at: Some(SystemTime::now()),
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::from([Capability::Inspect]),
        }
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let mut output = String::new();
        for y in buffer.area.top()..buffer.area.bottom() {
            for x in buffer.area.left()..buffer.area.right() {
                output.push_str(buffer.get(x, y).symbol());
            }
            output.push('\n');
        }
        output
    }
}
