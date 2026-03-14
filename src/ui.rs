use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap},
    Frame,
};

use crate::app::{App, TaskStatus};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header + tabs
            Constraint::Min(8),    // task list
            Constraint::Length(10), // log
            Constraint::Length(2), // help bar
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_tasks(f, app, chunks[1]);
    draw_log(f, app, chunks[2]);
    draw_help(f, app, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let tab_titles: Vec<Line> = app.tabs.iter().map(|t| Line::from(t.name)).collect();

    let title = if app.dry_run {
        " detox-mac [SIMULATION] "
    } else {
        " detox-mac "
    };

    let title_style = if app.dry_run {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };

    let tabs = Tabs::new(tab_titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(title_style),
        )
        .select(app.active_tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "));

    f.render_widget(tabs, area);
}

fn draw_tasks(f: &mut Frame, app: &App, area: Rect) {
    let tab = &app.tabs[app.active_tab];

    let items: Vec<ListItem> = tab
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let checkbox = if task.checked { "[x]" } else { "[ ]" };

            let status_icon = match task.status {
                TaskStatus::Pending => " ",
                TaskStatus::Done => " ",
                TaskStatus::Error => " ",
            };

            let is_selected = i == tab.selected;

            let style = match task.status {
                TaskStatus::Done => Style::default().fg(Color::Green),
                TaskStatus::Error => Style::default().fg(Color::Red),
                TaskStatus::Pending if is_selected => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                TaskStatus::Pending => Style::default().fg(Color::White),
            };

            let pointer = if is_selected { "▸ " } else { "  " };
            let root_badge = if task.needs_root { " [sudo]" } else { "" };

            // Badge confirmation
            let confirm_badge = if app.confirming && is_selected {
                " ⚠ Confirmer? (Entrée/Esc)"
            } else {
                ""
            };

            let line = Line::from(vec![
                Span::styled(pointer, style),
                Span::styled(format!("{} ", checkbox), Style::default().fg(Color::DarkGray)),
                Span::styled(status_icon, style),
                Span::styled(task.name, style),
                Span::styled(
                    root_badge,
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    confirm_badge,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);

            ListItem::new(vec![
                line,
                Line::from(vec![
                    Span::raw("      "),
                    Span::styled(task.description, Style::default().fg(Color::DarkGray)),
                ]),
            ])
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", tab.name.trim()))
            .title_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(list, area);
}

fn draw_log(f: &mut Frame, app: &App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let total = app.log.len();

    let start = if total <= inner_height {
        0
    } else {
        app.log_scroll.min(total.saturating_sub(inner_height))
    };

    let visible_lines: Vec<Line> = app.log[start..]
        .iter()
        .take(inner_height)
        .map(|msg| {
            let color = if msg.contains("Erreur") || msg.contains("Nécessite sudo") {
                Color::Red
            } else if msg.contains("SIMULATION") {
                Color::Yellow
            } else if msg.contains("OK")
                || msg.contains("libéré")
                || msg.contains("vidée")
                || msg.contains("purgée")
                || msg.contains("réinitialisé")
                || msg.contains("Terminé")
                || msg.contains("supprimé")
                || msg.contains("désactivé")
                || msg.contains("réactivé")
            {
                Color::Green
            } else if msg.starts_with("───") || msg.starts_with("═══") || msg.starts_with("──") {
                Color::Cyan
            } else if msg.contains("Confirmer") || msg.contains("Annulé") {
                Color::Yellow
            } else {
                Color::Gray
            };
            Line::from(Span::styled(msg.as_str(), Style::default().fg(color)))
        })
        .collect();

    let log_title = if app.dry_run {
        " Journal [SIMULATION] "
    } else {
        " Journal "
    };

    let log_block = Paragraph::new(visible_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(log_title)
                .title_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(log_block, area);
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Nav  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Espace ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Sél  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Entrée ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Exec  ", Style::default().fg(Color::DarkGray)),
        Span::styled("r ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Tous  ", Style::default().fg(Color::DarkGray)),
        Span::styled("◄► ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Onglet  ", Style::default().fg(Color::DarkGray)),
        Span::styled("d ", Style::default().fg(Color::Yellow).bold()),
    ];

    if app.dry_run {
        spans.push(Span::styled(
            "Simulation ON  ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled("Simulation  ", Style::default().fg(Color::DarkGray)));
    }

    spans.extend([
        Span::styled("PgUp/Dn ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q ", Style::default().fg(Color::Yellow).bold()),
        Span::styled("Quitter", Style::default().fg(Color::DarkGray)),
    ]);

    let help_text = Line::from(spans);
    let help = Paragraph::new(help_text).block(Block::default().borders(Borders::TOP));

    f.render_widget(help, area);
}
