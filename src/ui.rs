use std::time::{Duration, SystemTime};

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Frame, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{
    is_active_session_state, project_group_path, App, ComposerMode, ConfirmTarget, Overlay,
    SelectionKey, ViewMode,
};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionState};

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
        let message = if area.width >= 18 && area.height >= 2 {
            vec![Line::from("coding-agents needs"), Line::from("at least 32×8")]
        } else {
            vec![Line::from("needs 32×8")]
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().bg(BG).fg(ATTENTION))
                .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let composer_height = match app.overlay {
        Overlay::Peek => (6 + input_line_count(&app.input).saturating_sub(1))
            .min(10)
            .min(area.height.saturating_sub(5)),
        Overlay::Help => {
            let help_lines = pack_help_actions(
                help_actions(app),
                area.width.saturating_sub(2) as usize,
            )
            .len() as u16;
            (3 + help_lines).min(area.height.saturating_sub(5))
        }
        Overlay::Composer(_) => (3 + input_line_count(&app.input).saturating_sub(1))
            .min(7)
            .min(area.height.saturating_sub(5)),
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
                format!(
                    "{}{}",
                    sanitize_inline(&group.label),
                    suffix.unwrap_or_default()
                ),
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
        if app.filter.is_empty() && area.width >= 60 && area.height >= 12 {
            lines.extend(empty_state_lines());
        } else {
            lines.push(Line::from(Span::styled(
                if app.filter.is_empty() {
                    "No coding-agent sessions found"
                } else {
                    "No sessions match the current filter"
                },
                Style::default().fg(DIM),
            )));
        }
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

fn empty_state_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Needs input",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Sessions that have a question or need your decision land here",
            Style::default().fg(DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Working",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Sessions your coding agents are actively working on",
            Style::default().fg(DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Completed",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Finished sessions wait here for you to review",
            Style::default().fg(DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Hand off a substantial task below. Open Agent View will organize it by status so you can see when it needs you.",
            Style::default().fg(DIM),
        )),
    ]
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
    let metadata = compact_runtime_marker(session);
    let state_prefix = (view_mode == ViewMode::Directory)
        .then(|| format!("{} · ", short_state(session.state)))
        .unwrap_or_default();
    let age = format_age(session.age(SystemTime::now()));
    let prs = session
        .pull_requests
        .map(|count| format!("{count} PR{}", if count == 1 { "" } else { "s" }))
        .unwrap_or_default();
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
    let name = pad_to_width(truncate(&session.name, name_width), name_width);
    let summary = pad_to_width(summary, summary_width);
    let spans = vec![
        Span::styled(format!(" {symbol} "), symbol_style),
        Span::styled(
            name,
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{metadata} "), Style::default().fg(DIM)),
        Span::styled(summary, Style::default().fg(DIM)),
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
    let content_lines = if editable {
        input_lines(content)
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                Line::from(vec![
                    Span::styled(
                        if index == 0 { prefix } else { "" },
                        Style::default().fg(FG),
                    ),
                    Span::styled(line, text_style),
                ])
            })
            .collect::<Vec<_>>()
    } else {
        vec![Line::from(vec![
            Span::styled(prefix, Style::default().fg(FG)),
            Span::styled(content.to_owned(), text_style),
        ])]
    };
    frame.render_widget(Paragraph::new(content_lines).block(block), area);
    if editable {
        let last_line = app.input.rsplit('\n').next().unwrap_or_default();
        let line_index = input_line_count(&app.input)
            .saturating_sub(1)
            .min(area.height.saturating_sub(3));
        let prefix_width = (line_index == 0).then(|| prefix.chars().count()).unwrap_or(0);
        let cursor_x = area.x + 1 + prefix_width as u16 + display_width(last_line) as u16;
        frame.set_cursor(
            cursor_x.min(area.right().saturating_sub(1)),
            area.y + 1 + line_index,
        );
    }
}

fn render_peek(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(session) = app.selected_session() else {
        render_composer(frame, app, area);
        return;
    };
    let block = Block::default()
        .title(sanitize_inline(&format!(
            " {} · {} · {} ",
            session.name,
            session.provider,
            session.runtime.label()
        )))
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
    let can_approve = session.capabilities.contains(&Capability::Approve);
    let can_decline = session.capabilities.contains(&Capability::Decline);
    let can_respond = session.capabilities.contains(&Capability::Respond);
    let can_reply = session.capabilities.contains(&Capability::Reply);
    let response = if can_approve || can_decline {
        let choices = match (can_approve, can_decline) {
            (true, true) => "y allow once · n deny",
            (true, false) => "y allow once",
            (false, true) => "n deny",
            (false, false) => unreachable!(),
        };
        vec![Line::from(Span::styled(
            choices,
            Style::default().fg(ATTENTION),
        ))]
    } else if can_respond {
        editable_response_lines("answer", &app.input)
    } else if can_reply {
        editable_response_lines("reply", &app.input)
    } else {
        vec![Line::from(Span::styled(
            "enter to open native session",
            Style::default().fg(DIM),
        ))]
    };
    let summary_capacity = area
        .height
        .saturating_sub(3 + response.len() as u16)
        .max(1) as usize;
    let summary = sanitize_multiline(summary);
    let summary_lines: Vec<_> = summary.lines().collect();
    let summary_start = summary_lines.len().saturating_sub(summary_capacity);
    let mut lines: Vec<Line<'_>> = summary_lines[summary_start..]
        .iter()
        .map(|line| Line::from((*line).to_owned()))
        .collect();
    lines.push(Line::default());
    lines.extend(response);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
    if can_respond || can_reply {
        let last_line = app.input.rsplit('\n').next().unwrap_or_default();
        let prefix_width = if app.input.contains('\n') { 0 } else { 2 };
        frame.set_cursor(
            (area.x + 1 + prefix_width + display_width(last_line) as u16)
                .min(area.right().saturating_sub(2)),
            area.bottom().saturating_sub(2),
        );
    }
}

fn compact_runtime_marker(session: &AgentSession) -> &'static str {
    match (&session.provider, &session.runtime) {
        (Provider::Claude, Runtime::Host) => "C@H",
        (Provider::Claude, Runtime::Docker { .. }) => "C@D",
        (Provider::Codex, Runtime::Host) => "X@H",
        (Provider::Codex, Runtime::Docker { .. }) => "X@D",
        (Provider::Other(_), Runtime::Host) => "?@H",
        (Provider::Other(_), Runtime::Docker { .. }) => "?@D",
    }
}

fn editable_response_lines(label: &str, input: &str) -> Vec<Line<'static>> {
    if input.is_empty() {
        return vec![Line::from(Span::styled(
            format!("❯ {label}"),
            Style::default().fg(DIM),
        ))];
    }
    input_lines(input)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            Line::from(Span::styled(
            format!(
                "{}{}",
                if index == 0 { "❯ " } else { "" },
                sanitize_inline(&line)
            ),
                Style::default().fg(FG),
            ))
        })
        .collect()
}

fn input_lines(input: &str) -> Vec<String> {
    let lines = input.split('\n').collect::<Vec<_>>();
    lines
        .iter()
        .skip(lines.len().saturating_sub(5))
        .copied()
        .map(ToOwned::to_owned)
        .collect()
}

fn input_line_count(input: &str) -> u16 {
    input.split('\n').count().min(5) as u16
}

fn render_help(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lines = pack_help_actions(help_actions(app), area.width.saturating_sub(2) as usize)
        .into_iter()
        .map(|line| Line::from(format!("  {}", sanitize_inline(&line))))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(BG).fg(DIM)),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let footer = if let Some(notice) = &app.notice {
        sanitize_inline(notice)
    } else if let Some(warning) = app.snapshot.warnings.first() {
        format!("warning: {}", sanitize_inline(warning))
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
        Overlay::Peek
            if app.selected_session().is_some_and(|session| {
                session.capabilities.contains(&Capability::Approve)
                    || session.capabilities.contains(&Capability::Decline)
            }) =>
        {
            let session = app.selected_session().expect("selection checked");
            match (
                session.capabilities.contains(&Capability::Approve),
                session.capabilities.contains(&Capability::Decline),
            ) {
                (true, true) => "y to allow once · n to deny · esc to close".into(),
                (true, false) => "y to allow once · esc to close".into(),
                (false, true) => "n to deny · esc to close".into(),
                (false, false) => unreachable!(),
            }
        }
        Overlay::Peek
            if app
                .selected_session()
                .is_some_and(|session| session.capabilities.contains(&Capability::Respond)) =>
        {
            "enter to submit answer · esc to close".into()
        }
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
    if session.capabilities.contains(&Capability::Approve)
        || session.capabilities.contains(&Capability::Decline)
    {
        " · space to review request"
    } else if session.capabilities.contains(&Capability::Respond) {
        " · space to answer"
    } else if session.capabilities.contains(&Capability::Reply) {
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
        if session.capabilities.contains(&Capability::Approve)
            || session.capabilities.contains(&Capability::Decline)
        {
            actions.push("space to review request".into());
            if session.capabilities.contains(&Capability::Approve) {
                actions.push("y to allow once".into());
            }
            if session.capabilities.contains(&Capability::Decline) {
                actions.push("n to deny".into());
            }
        } else if session.capabilities.contains(&Capability::Respond) {
            actions.push("space to answer request".into());
        } else if session.capabilities.contains(&Capability::Reply) {
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
        Paragraph::new(sanitize_multiline(&message))
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
        sanitize_inline(&labels.join(" + "))
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
    let input = sanitize_inline(input);
    if display_width(&input) <= width {
        return input;
    }
    if width <= 1 {
        return "…".into();
    }
    let mut output = String::new();
    let mut used = 0;
    for grapheme in input.graphemes(true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width + 1 > width {
            break;
        }
        output.push_str(grapheme);
        used += grapheme_width;
    }
    output.push('…');
    output
}

fn pad_to_width(mut input: String, width: usize) -> String {
    let padding = width.saturating_sub(display_width(&input));
    input.extend(std::iter::repeat(' ').take(padding));
    input
}

fn display_width(input: &str) -> usize {
    Line::from(input).width()
}

fn sanitize_inline(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

fn sanitize_multiline(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            '\n' => '\n',
            '\t' => ' ',
            character if character.is_control() => '�',
            character => character,
        })
        .collect()
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
        .map(|path| sanitize_inline(&abbreviate_path(&path)))
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
    fn compact_row_marker_preserves_summary_space_and_peek_shows_full_runtime() {
        let mut item = session("worker", SessionState::Working);
        item.provider = Provider::Codex;
        item.runtime = Runtime::Docker {
            container_id: "a".repeat(64),
            container_name: "long-container-name-that-must-not-shrink-the-row".into(),
            image: "example/image@sha256:digest".into(),
        };
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let row = buffer_text(terminal.backend().buffer());
        assert!(row.contains("X@D"));
        assert!(row.contains("latest summary from worker"));
        assert!(!row.contains("long-container-name-that-must-not-shrink-the-row"));

        app.toggle_peek();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let peek = buffer_text(terminal.backend().buffer());
        assert!(peek.contains("worker · Codex · long-container-name"));
    }

    #[test]
    fn provider_text_cannot_emit_terminal_controls_and_wide_text_stays_bounded() {
        let mut item = session("unsafe\u{1b}[31m\nname", SessionState::Working);
        item.summary = "deploy\u{7} summary".into();
        item.runtime = Runtime::Docker {
            container_id: "a".repeat(64),
            container_name: "container\u{1b}[2J".into(),
            image: "example/image@sha256:digest".into(),
        };
        let app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec!["warning\u{1b}[2J\ncontinued".into()],
        });
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains('�'));
        assert!(rendered.contains("warning�[2J�continued"));

        let truncated = truncate("部署版本", 5);
        assert_eq!(truncated, "部署…");
        assert_eq!(display_width(&truncated), 5);
        assert_eq!(display_width(&pad_to_width(truncate("部署", 8), 8)), 8);
    }

    #[test]
    fn tiny_terminals_render_a_clear_minimum_size_message() {
        let app = App::new(SessionSnapshot::default());
        let backend = TestBackend::new(31, 7);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("coding-agents needs"));
        assert!(rendered.contains("at least 32×8"));
    }

    #[test]
    fn empty_dashboard_preserves_the_reference_section_anatomy() {
        let app = App::new(SessionSnapshot::default());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Needs input"));
        assert!(rendered.contains("question or need your decision"));
        assert!(rendered.contains("Working"));
        assert!(rendered.contains("actively working"));
        assert!(rendered.contains("Completed"));
        assert!(rendered.contains("wait here for you to review"));
        assert!(rendered.contains("Hand off a substantial task below"));
    }

    #[test]
    fn an_empty_filter_result_does_not_show_onboarding_copy() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("worker", SessionState::Working)],
            warnings: vec![],
        });
        app.filter = "no-match".into();
        app.replace_snapshot(app.snapshot.clone());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("No sessions match the current filter"));
        assert!(!rendered.contains("Hand off a substantial task below"));
    }

    #[test]
    fn multiline_composer_expands_and_renders_each_input_line() {
        let mut app = App::new(SessionSnapshot::default());
        app.start_new_session(None);
        for character in "first line\nsecond line\nthird line".chars() {
            app.push_input(character);
        }
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("❯ first line"));
        assert!(rendered.contains("second line"));
        assert!(rendered.contains("third line"));
        assert!(rendered.contains("ctrl+j for newline"));
    }

    #[test]
    fn multiline_peek_keeps_the_latest_summary_and_draft_visible() {
        let mut item = session("worker", SessionState::Working);
        item.capabilities.insert(Capability::Reply);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_peek();
        app.set_detail(
            "worker".into(),
            "old line\nnew provider detail".into(),
        );
        for character in "first reply line\nsecond reply line".chars() {
            app.push_input(character);
        }
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("new provider detail"));
        assert!(rendered.contains("❯ first reply line"));
        assert!(rendered.contains("second reply line"));
        assert!(rendered.contains("enter to send"));
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

    #[test]
    fn denial_only_request_never_advertises_acceptance() {
        let mut item = session("blocked", SessionState::NeedsInput);
        item.capabilities.insert(Capability::Decline);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_peek();
        app.set_detail(
            "blocked".into(),
            "Codex requests file-change approval without a diff.".into(),
        );
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("n deny"));
        assert!(!rendered.contains("y allow once"));
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
