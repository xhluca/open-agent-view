use std::time::{Duration, SystemTime};

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Frame, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{
    is_active_session_state, project_group_path, App, ComposerMode, ConfirmTarget, Overlay,
    SelectionKey, ViewMode, MODEL_PICKER_PAGE_SIZE, SESSION_PAGE_SIZE,
};
use crate::domain::{AgentSession, Capability, SessionState};

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
            vec![
                Line::from("coding-agents needs"),
                Line::from("at least 32×8"),
            ]
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
            let help_lines =
                pack_help_actions(help_actions(app), area.width.saturating_sub(2) as usize).len()
                    as u16;
            (3 + help_lines).min(area.height.saturating_sub(5))
        }
        Overlay::HarnessPicker => (3 + input_line_count(&app.input).saturating_sub(1))
            .min(7)
            .min(area.height.saturating_sub(5)),
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

    if let Overlay::Confirm(target) = &app.overlay {
        render_confirmation(frame, app, target, area);
    } else if app.overlay == Overlay::HarnessPicker {
        render_harness_picker(frame, app, area);
    } else if app.overlay == Overlay::ModelPicker {
        render_model_picker(frame, app, area);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let cwd = header_directory(app);
    let (awaiting, working, completed) = app.snapshot.sessions.iter().fold(
        (0usize, 0usize, 0usize),
        |(awaiting, working, completed), session| match session.state {
            SessionState::NeedsInput => (awaiting + 1, working, completed),
            SessionState::ReadyForReview | SessionState::Working => {
                (awaiting, working + 1, completed)
            }
            SessionState::Completed => (awaiting, working, completed + 1),
            SessionState::Unknown => (awaiting, working, completed),
        },
    );
    let completed_status = if app.includes_completed {
        format!("{completed} completed (/completed hide)")
    } else {
        "completed hidden (/completed show)".into()
    };
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
            Line::from(format!(
                "{awaiting} awaiting · {working} working · {completed_status}"
            )),
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
                        "{awaiting} awaiting input · {working} working · {completed_status} · {mode} view"
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
                format!("{awaiting} awaiting · {working} working · {completed_status}"),
                Style::default().fg(DIM),
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(BG).fg(FG)),
        area,
    );
}

fn render_session_list(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut lines = Vec::new();
    let mut selected_line = None;
    let selection_visible = !matches!(
        app.overlay,
        Overlay::Composer(_) | Overlay::HarnessPicker | Overlay::ModelPicker
    );

    for (group_position, group) in app.groups().into_iter().enumerate() {
        if group_position > 0 {
            lines.push(Line::default());
        }
        let is_selected =
            selection_visible && app.selection == Some(SelectionKey::Group(group.key.clone()));
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
        let visible = app.visible_session_count(&group);
        for index in group.sessions.iter().take(visible) {
            let session = &app.snapshot.sessions[*index];
            let is_selected = selection_visible
                && app.selection == Some(SelectionKey::Session(session.id.clone()));
            if is_selected {
                selected_line = Some(lines.len());
            }
            lines.push(render_session_row(
                session,
                app.view_mode,
                area.width,
                is_selected,
            ));
        }
        let hidden = app.hidden_session_count(&group);
        if hidden > 0 {
            let is_selected = selection_visible
                && app.selection == Some(SelectionKey::ShowMore(group.key.clone()));
            if is_selected {
                selected_line = Some(lines.len());
            }
            lines.push(render_show_more_row(hidden, is_selected));
        }
    }

    if lines.is_empty() {
        if app.filter.is_empty() && area.width >= 60 && area.height >= 12 {
            lines.extend(empty_state_lines(app.includes_completed));
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

    // Keep the viewport page-aligned. A one-line sliding window rewrites every
    // visible row for every arrow repeat once the selection reaches the bottom,
    // which creates a large output backlog on SSH/tmux with long session lists.
    // Within a page, navigation now changes only the old and new selected rows.
    let page_height = area.height.max(1) as usize;
    let scroll = selected_line
        .map(|line| (line / page_height) * page_height)
        .unwrap_or(0)
        .min(u16::MAX as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(BG).fg(FG))
            .scroll((scroll as u16, 0)),
        area,
    );
}

fn render_show_more_row(hidden: usize, selected: bool) -> Line<'static> {
    let next = hidden.min(SESSION_PAGE_SIZE);
    styled_line(
        vec![
            Span::styled("  ↓ ", Style::default().fg(ACCENT)),
            Span::styled(
                format!("Show {next} more · {hidden} hidden"),
                Style::default().fg(DIM),
            ),
        ],
        selected,
    )
}

fn empty_state_lines(includes_completed: bool) -> Vec<Line<'static>> {
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
            if includes_completed {
                " Finished sessions wait here for you to review"
            } else {
                " Hidden by default · use /completed show (or start with --all)"
            },
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
    let name_width = if width >= 100 {
        26
    } else if width >= 70 {
        20
    } else if width >= 50 {
        16
    } else {
        10
    };
    // Provider identity is primary row information. Keep the complete names of
    // every built-in provider visible instead of requiring users to decode the
    // old C@H/X@D-style marker. Runtime details remain available in Peek.
    let provider_width = 14;
    let provider = pad_to_width(
        truncate(session.provider.label(), provider_width),
        provider_width,
    );
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
    let fixed = 5 + name_width + provider_width + right.len();
    let summary_width = (width as usize).saturating_sub(fixed).max(1);
    let summary = truncate(&format!("{state_prefix}{}", session.summary), summary_width);
    let name = pad_to_width(truncate(&session.name, name_width), name_width);
    let summary = pad_to_width(summary, summary_width);
    let spans = vec![
        Span::styled(format!(" {symbol} "), symbol_style),
        Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {provider} "), Style::default().fg(ACCENT)),
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
    let mut block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(BG));
    if matches!(
        app.overlay,
        Overlay::Composer(ComposerMode::NewSession)
            | Overlay::HarnessPicker
            | Overlay::ModelPicker
    ) {
        let model = app.launch_model.as_deref().unwrap_or("default");
        block = block.title(format!(
            " new task · harness {} · model {model} ",
            app.launch_provider.label()
        ));
    }
    let (prefix, content, editable) = match &app.overlay {
        Overlay::Composer(ComposerMode::NewSession) => ("❯ ", app.input.as_str(), true),
        Overlay::HarnessPicker | Overlay::ModelPicker => ("❯ ", app.input.as_str(), false),
        Overlay::Composer(ComposerMode::Rename { .. }) => ("❯ name ", app.input.as_str(), true),
        Overlay::Composer(ComposerMode::Filter) => ("❯ filter ", app.input.as_str(), true),
        _ => (
            "❯ ",
            if app.filter.is_empty() {
                "describe a task · /help for commands"
            } else {
                "describe a task · ctrl+f to change filter"
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
        let prefix_width = (line_index == 0)
            .then(|| display_width(prefix))
            .unwrap_or(0);
        let cursor_x = area.x + prefix_width as u16 + display_width(last_line) as u16;
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
    let summary_capacity = area.height.saturating_sub(3 + response.len() as u16).max(1) as usize;
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
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
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
        Paragraph::new(lines).style(Style::default().bg(BG).fg(DIM)),
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
        Overlay::Composer(ComposerMode::NewSession) if width >= 100 => {
            "enter to create · tab choose harness · /harness · /model · ctrl+j newline · esc cancel"
                .into()
        }
        Overlay::Composer(ComposerMode::NewSession) if width >= 70 => {
            "enter to create · tab choose harness · ctrl+j newline · /help · esc cancel".into()
        }
        Overlay::Composer(ComposerMode::NewSession) if width >= 55 => {
            "enter to create · tab harness · /help · esc cancel".into()
        }
        Overlay::Composer(ComposerMode::NewSession) => "enter to create · tab harness".into(),
        Overlay::HarnessPicker if width >= 55 => format!(
            "↑/↓ or tab to choose · 1–{} direct · enter select · esc back",
            app.launch_targets.len().min(9)
        ),
        Overlay::HarnessPicker => "↑/↓ choose · enter · esc".into(),
        Overlay::ModelPicker if width >= 70 => {
            "type to filter · ↑/↓ move · page up/down · enter select · esc back".into()
        }
        Overlay::ModelPicker => "type filter · ↑/↓ · enter · esc".into(),
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
        Overlay::None if matches!(app.selection, Some(SelectionKey::ShowMore(_))) => {
            if width >= 55 {
                "enter to show more · ↑/↓ to select · ? for shortcuts".into()
            } else {
                "enter to show more · ? for shortcuts".into()
            }
        }
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
        _ if width >= 90 => {
            "type to create · ↑/↓ select · ctrl+f filter · /completed show|hide · ? shortcuts"
                .into()
        }
        _ if width >= 70 => {
            "type to create · ↑/↓ to select · /completed · ? for shortcuts".into()
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
    if matches!(app.selection, Some(SelectionKey::ShowMore(_))) {
        actions.push("enter to show more".into());
    }
    if app.selected_session().is_some() {
        actions.push("ctrl+r to rename".into());
    }
    actions.push("ctrl+s to switch views".into());
    actions.push("ctrl+j for newline".into());
    actions.push("ctrl+f to filter".into());
    actions.push("ctrl+l to refresh".into());
    actions.push("tab for new task/harness picker".into());
    actions.push("/help for task commands".into());
    actions.push("/completed to show/hide finished sessions".into());
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

fn render_harness_picker(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.launch_targets.is_empty() {
        return;
    }
    let popup_width = area.width.saturating_sub(2).min(58).max(28);
    let desired_height = app.launch_targets.len() as u16 + 3;
    let popup_height = desired_height.min(area.height.saturating_sub(2)).max(5);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(popup_width) / 2,
        area.y + area.height.saturating_sub(popup_height) / 2,
        popup_width,
        popup_height,
    );
    let visible_rows = popup_height.saturating_sub(3).max(1) as usize;
    let start = (app.harness_selection / visible_rows) * visible_rows;
    let mut lines = app
        .launch_targets
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
        .map(|(index, target)| {
            let selected = index == app.harness_selection;
            let current = target.provider == app.launch_provider;
            let detail = if popup_width >= 46 {
                if target.supports_model {
                    "selectable model"
                } else {
                    "default model"
                }
            } else if target.supports_model {
                "model"
            } else {
                "default"
            };
            let label = format!(
                " {} {}  {:<16} {}{}",
                if selected { "›" } else { " " },
                index + 1,
                sanitize_inline(target.provider.label()),
                detail,
                if current { " · current" } else { "" }
            );
            Line::from(label).style(if selected {
                Style::default()
                    .bg(SELECTED_BG)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if current {
                Style::default().bg(BG).fg(ACCENT)
            } else {
                Style::default().bg(BG).fg(FG)
            })
        })
        .collect::<Vec<_>>();
    lines.push(
        Line::from(if popup_width >= 46 {
            " ↑/↓ or tab move · enter select · esc back"
        } else {
            " ↑/↓ · enter · esc"
        })
        .style(Style::default().fg(DIM)),
    );

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(
                        " choose harness · {}/{} ",
                        app.harness_selection + 1,
                        app.launch_targets.len()
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT)),
            )
            .style(Style::default().bg(BG).fg(FG)),
        popup,
    );
}

fn render_model_picker(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let choices = app.model_choices();
    let popup_width = area.width.saturating_sub(2).min(76).max(28);
    let visible_rows = MODEL_PICKER_PAGE_SIZE
        .min(area.height.saturating_sub(7).max(1) as usize)
        .max(1);
    let result_rows = choices.len().clamp(1, visible_rows);
    let popup_height = (result_rows as u16 + 5)
        .min(area.height.saturating_sub(2))
        .max(6);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(popup_width) / 2,
        area.y + area.height.saturating_sub(popup_height) / 2,
        popup_width,
        popup_height,
    );
    let start = if choices.is_empty() {
        0
    } else {
        (app.model_selection / visible_rows) * visible_rows
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(" filter  ", Style::default().fg(DIM)),
        Span::styled(
            if app.model_filter.is_empty() {
                "type to search".into()
            } else {
                sanitize_inline(&app.model_filter)
            },
            if app.model_filter.is_empty() {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(FG)
            },
        ),
    ])];
    if choices.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.models_loading {
                "  Loading models…"
            } else {
                "  No matching models"
            },
            Style::default().fg(DIM),
        )));
    } else {
        lines.extend(
            choices
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_rows)
                .map(|(index, model)| {
                    let selected = index == app.model_selection;
                    let label = model.unwrap_or("Default");
                    let current = *model == app.launch_model.as_deref();
                    Line::from(format!(
                        " {} {}{}",
                        if selected { "›" } else { " " },
                        sanitize_inline(label),
                        if current { " · current" } else { "" }
                    ))
                    .style(if selected {
                        Style::default()
                            .bg(SELECTED_BG)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if current {
                        Style::default().bg(BG).fg(ACCENT)
                    } else {
                        Style::default().bg(BG).fg(FG)
                    })
                }),
        );
    }
    if app.models_loading && !choices.is_empty() {
        lines.push(Line::from(Span::styled(
            " Discovering available models…",
            Style::default().fg(DIM),
        )));
    }
    lines.push(
        Line::from(if popup_width >= 58 {
            " ↑/↓ move · PgUp/PgDn page · enter select · esc back"
        } else {
            " ↑/↓ · enter · esc"
        })
        .style(Style::default().fg(DIM)),
    );

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(
                        " choose {} model · {} result{} ",
                        app.launch_provider.label(),
                        choices.len(),
                        if choices.len() == 1 { "" } else { "s" }
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(ACCENT)),
            )
            .style(Style::default().bg(BG).fg(FG)),
        popup,
    );
    let filter_prefix = " filter  ";
    frame.set_cursor(
        (popup.x
            + 1
            + display_width(filter_prefix) as u16
            + display_width(&app.model_filter) as u16)
            .min(popup.right().saturating_sub(2)),
        popup.y + 1,
    );
}

fn render_confirmation(frame: &mut Frame<'_>, _: &App, target: &ConfirmTarget, area: Rect) {
    let popup = centered_rect(72, 30, area);
    frame.render_widget(Clear, popup);
    let message = match target {
        ConfirmTarget::OpenClaude { id } => format!(
            "Attach to this Claude background session?\n\n{id}\n\nInside Claude:\n← opens Claude's agent view.\nCtrl+Z returns to Open Agent View.\nThe background session keeps running.\n\nEnter attaches; escape cancels."
        ),
        ConfirmTarget::Session { id, running: true } => {
            format!(
                "Interrupt the exact running session?\n\n{id}\n\nEnter confirms; escape keeps it."
            )
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
    let labels = app
        .snapshot
        .sessions
        .iter()
        .map(|session| session.provider.label())
        .collect::<std::collections::BTreeSet<_>>();
    if labels.is_empty() {
        "Coding agents".into()
    } else {
        sanitize_inline(&labels.into_iter().collect::<Vec<_>>().join(" + "))
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
        AgentSession, Capability, LaunchTarget, Provider, Runtime, SessionKind, SessionSnapshot,
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
        assert!(rendered.contains("describe a task · /help for commands"));
        assert!(rendered.contains("1 awaiting input · 2 working · 1 completed"));
    }

    #[test]
    fn header_makes_default_completed_filter_explicit() {
        let app = App::with_completed_visibility(
            SessionSnapshot {
                sessions: vec![],
                warnings: vec![],
            },
            false,
        );
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("completed hidden"));
        assert!(!rendered.contains("0 completed"));
        assert!(rendered.contains("/completed show"));
    }

    #[test]
    fn large_groups_render_a_bounded_page_and_show_more_control() {
        let sessions = (0..60)
            .map(|index| session(&format!("session-{index:02}"), SessionState::Working))
            .collect();
        let mut app = App::new(SessionSnapshot {
            sessions,
            warnings: vec![],
        });
        let backend = TestBackend::new(120, 70);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let first_page = buffer_text(terminal.backend().buffer());
        assert!(first_page.contains("session-00"));
        assert!(first_page.contains("session-24"));
        assert!(!first_page.contains("session-25"));
        assert!(first_page.contains("Show 25 more · 35 hidden"));

        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        assert_eq!(app.activate(), crate::app::AppAction::None);
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let second_page = buffer_text(terminal.backend().buffer());
        assert!(second_page.contains("session-25"));
        assert!(second_page.contains("session-49"));
        assert!(!second_page.contains("session-50"));
        assert!(second_page.contains("Show 10 more · 10 hidden"));
        assert!(second_page.contains("enter to open"));

        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let selected_control = buffer_text(terminal.backend().buffer());
        assert!(selected_control.contains("enter to show more"));
    }

    #[test]
    fn row_names_provider_and_peek_shows_full_runtime() {
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
        assert!(row.contains("Codex"));
        assert!(!row.contains("X@D"));
        assert!(row.contains("latest summary from worker"));
        assert!(!row.contains("long-container-name-that-must-not-shrink-the-row"));

        app.toggle_peek();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let peek = buffer_text(terminal.backend().buffer());
        assert!(peek.contains("worker · Codex · long-container-name"));
    }

    #[test]
    fn every_builtin_provider_is_named_on_its_session_row() {
        let providers = [
            (Provider::Claude, "Claude"),
            (Provider::Codex, "Codex"),
            (Provider::Pi, "Pi"),
            (Provider::OpenCode, "OpenCode"),
            (Provider::Cursor, "Cursor"),
            (Provider::GitHubCopilot, "GitHub Copilot"),
            (Provider::Antigravity, "Antigravity"),
        ];

        for (provider, expected) in providers {
            let mut item = session("recognizable-session", SessionState::Working);
            item.provider = provider;
            let row = render_session_row(&item, ViewMode::Status, 120, false);
            let text: String = row.spans.iter().map(|span| span.content.as_ref()).collect();

            assert!(
                text.contains(expected),
                "session row did not name provider {expected}: {text:?}"
            );
            assert!(!text.contains('@'));
        }
    }

    #[test]
    fn truncated_session_name_keeps_a_gap_before_provider() {
        let mut item = session(
            "session-name-that-is-much-longer-than-the-column",
            SessionState::Working,
        );
        item.provider = Provider::Antigravity;
        let row = render_session_row(&item, ViewMode::Status, 120, false);
        let text: String = row.spans.iter().map(|span| span.content.as_ref()).collect();

        assert!(
            text.contains("… Antigravity"),
            "provider column touched name: {text:?}"
        );
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
        assert!(rendered.contains("ctrl+j newline"));
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
        app.set_detail("worker".into(), "old line\nnew provider detail".into());
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
        assert!(rendered.contains("describe a task · /help for commands"));
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

    #[test]
    fn new_task_composer_names_the_selected_provider_and_model() {
        let mut app = App::new(SessionSnapshot::default());
        app.launch_model = Some("opus".into());
        app.start_new_session(None);
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("new task · harness Claude · model opus"));
        assert!(rendered.contains("tab choose harness"));
        assert!(rendered.contains("/harness"));
        assert!(rendered.contains("/model"));
    }

    #[test]
    fn composer_cursor_is_exactly_after_the_text_without_a_phantom_left_border() {
        let mut app = App::new(SessionSnapshot::default());
        app.start_new_session(None);
        app.input = "abc".into();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();

        assert_eq!(terminal.get_cursor().unwrap(), (5, 21));
        assert_eq!(terminal.backend().buffer().get(2, 21).symbol(), "a");
        assert_eq!(terminal.backend().buffer().get(4, 21).symbol(), "c");
    }

    #[test]
    fn model_picker_is_searchable_bounded_and_keeps_the_selected_page_visible() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Pi,
            vec![LaunchTarget {
                provider: Provider::Pi,
                supports_model: true,
            }],
        );
        app.start_new_session(None);
        app.input = "draft survives model selection".into();
        app.open_model_picker();
        app.set_available_models(
            Provider::Pi,
            Ok((0..25).map(|index| format!("provider/model-{index:02}")).collect()),
        );
        app.model_selection = 13;
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let page = buffer_text(terminal.backend().buffer());
        assert!(page.contains("choose Pi model · 26 results"));
        assert!(page.contains("provider/model-09"));
        assert!(page.contains("provider/model-18"));
        assert!(!page.contains("provider/model-08"));
        assert!(!page.contains("provider/model-19"));
        assert!(page.contains("PgUp/PgDn"));
        assert!(page.contains("draft survives model selection"));

        app.push_input('2');
        app.push_input('4');
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let filtered = buffer_text(terminal.backend().buffer());
        assert!(filtered.contains("filter  24"));
        assert!(filtered.contains("choose Pi model · 1 result"));
        assert!(filtered.contains("provider/model-24"));
        assert!(!filtered.contains("provider/model-23"));
        assert_eq!(terminal.get_cursor().unwrap(), (24, 13));
    }

    #[test]
    fn harness_picker_lists_every_available_choice_and_marks_the_current_one() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Codex,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::OpenCode,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::Cursor,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::GitHubCopilot,
                    supports_model: false,
                },
            ],
        );
        app.start_new_session(None);
        app.input = "draft survives".into();
        app.open_harness_picker();
        app.move_harness_selection(1);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("choose harness · 2/6"));
        assert!(rendered.contains("1  Claude"));
        assert!(rendered.contains("2  Codex"));
        assert!(rendered.contains("3  Pi"));
        assert!(rendered.contains("4  OpenCode"));
        assert!(rendered.contains("5  Cursor"));
        assert!(rendered.contains("6  GitHub Copilot"));
        assert!(rendered.contains("selectable model"));
        assert!(rendered.contains("default model"));
        assert!(rendered.contains("draft survives"));
        assert!(rendered.contains("enter select"));
        assert!(rendered.contains("esc back"));
    }

    #[test]
    fn narrow_harness_picker_pages_to_keep_the_highlighted_choice_visible() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Codex,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::OpenCode,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::Cursor,
                    supports_model: false,
                },
                LaunchTarget {
                    provider: Provider::GitHubCopilot,
                    supports_model: false,
                },
            ],
        );
        app.start_new_session(None);
        app.open_harness_picker();
        app.harness_selection = 5;
        let backend = TestBackend::new(32, 8);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("choose harness · 6/6"));
        assert!(rendered.contains("6  GitHub Copilot"));
        assert!(!rendered.contains("1  Claude"));
    }

    #[test]
    fn claude_attach_confirmation_explains_the_only_return_key() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("worker", SessionState::Working)],
            warnings: vec![],
        });
        assert_eq!(app.activate(), crate::app::AppAction::None);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Attach to this Claude background session?"));
        assert!(rendered.contains("Ctrl+Z returns to Open Agent View"));
        assert!(rendered.contains("← opens Claude's agent view"));
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
