//! Drawing the terminal: a function of [`App`] state, nothing else.

use std::sync::atomic::{AtomicBool, Ordering};

use jiff::Timestamp as JiffTimestamp;
use jiff::tz::TimeZone;
use liveside_proto::model::{
    Badge, ChatMessage, Fragment, Line, LineContent, LineFlags, NoticeLevel, ReplyTo, RoomModes,
    User,
};
use liveside_proto::{ConnectionState, Rgb, Timestamp};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Clear, Dataset, GraphType, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use liveside_client::days;
use liveside_client::lang::{counted, fill};
use liveside_client::settings::Value;

/// Whether the terminal's background is light. Colours are corrected
/// for the background they are drawn on, and a terminal does not say
/// which it has unless asked, so this is decided once at start (from
/// the settings, or by asking the terminal) and changed when the
/// setting is.
static LIGHT_BACKGROUND: AtomicBool = AtomicBool::new(false);

pub fn set_light_background(light: bool) {
    LIGHT_BACKGROUND.store(light, Ordering::Relaxed);
}

fn light_background() -> bool {
    LIGHT_BACKGROUND.load(Ordering::Relaxed)
}

/// Where a link landed on screen: one row's piece of it, since a link can
/// be wrapped. `id` is the same for every piece of one link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkArea {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub url: String,
    pub id: String,
}

/// Draws the screen, and says where the links in the chat are, so the
/// caller can make them clickable (OSC 8) after ratatui has drawn: written
/// into the cells, the escape sequences would count as text.
pub fn draw(frame: &mut Frame, app: &App) -> Vec<LinkArea> {
    let [header, chat, prompt] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(header_line(app, header.width), header);
    // The user list takes a column on the right, as in every chat client
    // with one; a narrow window keeps the chat instead.
    let users_width = users_width(app, chat.width);
    let mut links = if users_width > 0 {
        let [chat, users] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(users_width)]).areas(chat);
        let links = draw_chat(frame, app, chat);
        draw_users(frame, app, users);
        links
    } else {
        draw_chat(frame, app, chat)
    };
    // The settings screen covers the chat, as a dialog would in a GUI,
    // and the links under it with it.
    let graph = app.graph.as_ref().and_then(|channel| app.index_of(channel));
    if app.settings.open {
        draw_settings(frame, app, frame.area());
        links.clear();
    } else if graph.is_none() {
        // The prompt is where the cursor goes; under a screen that covers
        // it, the cursor stood on that screen's frame.
        draw_prompt(frame, app, prompt);
    }
    if let Some(i) = graph {
        draw_graph(frame, app, &app.buffers[i], frame.area());
        links.clear();
    }
    if light_background() {
        for cell in &mut frame.buffer_mut().content {
            if cell.bg == Color::Reset {
                cell.fg = on_light(cell.fg);
            }
        }
    }
    links
}

/// A colour of the terminal's palette as it is drawn on a light
/// background. The palette is the terminal's to choose, but its light
/// greys and pale tints are made for a dark one and vanish on white;
/// their deeper siblings stay readable on either. Only text on the
/// terminal's own background is changed: on a coloured one, the pair was
/// chosen together.
fn on_light(color: Color) -> Color {
    match color {
        Color::Gray => Color::DarkGray,
        Color::White => Color::Black,
        Color::LightRed => Color::Red,
        Color::LightGreen => Color::Green,
        Color::LightYellow => Color::Yellow,
        Color::LightBlue => Color::Blue,
        Color::LightMagenta => Color::Magenta,
        Color::LightCyan => Color::Cyan,
        other => other,
    }
}

/// How many watched a stream, drawn in braille: each character is a grid
/// of two by four dots, which makes a terminal fine enough for a line.
/// The changes of title or category stand up from the axis in another
/// colour and are listed, numbered, under it.
fn draw_graph(frame: &mut Frame, app: &App, buffer: &crate::app::Buffer, area: Rect) {
    use liveside_client::lang::{grouped, stream_mark, viewer_summary};
    let text = app.text();
    let history = &buffer.viewers;
    const LISTED: usize = 5;
    let marks_shown = history.marks.len().min(LISTED);
    let width = area.width.saturating_sub(4).min(140);
    let height = area
        .height
        .saturating_sub(2)
        .min(22 + u16::try_from(marks_shown).unwrap_or(0));
    let box_area = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    clear_for_box(frame, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(fill(
            text.graph_title,
            &[("channel", buffer.title.trim_start_matches('#'))],
        ));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let footer = 1 + if marks_shown > 0 {
        marks_shown as u16 + 1
    } else {
        0
    };
    let [summary, chart, below] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(footer),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            " {}",
            viewer_summary(text, history, clock).replace('\u{a0}', " ")
        ))
        .style(Style::new().add_modifier(Modifier::BOLD)),
        summary,
    );

    let mut lines: Vec<TextLine> = Vec::new();
    if marks_shown > 0 {
        lines.push(TextLine::from(Span::styled(
            format!(" {}", text.graph_marks),
            Style::new().fg(Color::DarkGray),
        )));
        let first = history.marks.len() - marks_shown;
        for (i, mark) in history.marks.iter().enumerate().skip(first) {
            lines.push(TextLine::from(vec![
                Span::styled(format!(" {}. ", i + 1), Style::new().fg(Color::Magenta)),
                Span::raw(format!("{} {}", clock(mark.time), stream_mark(mark))),
            ]));
        }
    }
    lines.push(TextLine::from(Span::styled(
        text.graph_keys,
        Style::new().fg(Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(lines), below);

    let (Some(first), Some(last), Some(peak)) = (
        history.samples.first(),
        history.samples.last(),
        history.peak(),
    ) else {
        return;
    };
    // Minutes since the first sample across, viewers up; one sample is
    // still a minute wide, so it shows as a short level line.
    let from = first.time.0;
    let to = last.time.0.max(from + 60_000);
    let minutes = |t: i64| (t - from) as f64 / 60_000.0;
    let mut points: Vec<(f64, f64)> = history
        .samples
        .iter()
        .map(|s| (minutes(s.time.0), f64::from(s.viewers)))
        .collect();
    if points.len() == 1 {
        points.push((minutes(to), points[0].1));
    }
    let top = round_up(f64::from(peak.viewers) * 1.1);
    let marks: Vec<[(f64, f64); 2]> = history
        .marks
        .iter()
        .map(|m| {
            let x = minutes(m.time.0.clamp(from, to));
            [(x, 0.0), (x, top)]
        })
        .collect();
    let mut datasets: Vec<Dataset> = marks
        .iter()
        .map(|line| {
            Dataset::default()
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(Color::Magenta))
                .data(line)
        })
        .collect();
    datasets.push(
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(Color::Cyan))
            .data(&points),
    );
    let label = |v: f64| grouped(v as u32).replace('\u{a0}', " ");
    let graph = Chart::new(datasets)
        .x_axis(
            Axis::default()
                .style(Style::new().fg(Color::DarkGray))
                .bounds([0.0, minutes(to)])
                .labels([
                    clock(Timestamp(from)),
                    clock(Timestamp((from + to) / 2)),
                    clock(Timestamp(to)),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::new().fg(Color::DarkGray))
                .bounds([0.0, top])
                .labels([label(0.0), label(top / 2.0), label(top)]),
        );
    frame.render_widget(graph, chart_area(chart));
}

/// The chart's room, a column in from the frame on the right.
fn chart_area(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1),
        ..area
    }
}

/// A round number at or above `value` for the top of the axis: 1, 2 or 5
/// times a power of ten, so its half is round too.
fn round_up(value: f64) -> f64 {
    let value = value.max(1.0);
    let power = 10f64.powf(value.log10().floor());
    for step in [1.0, 2.0, 5.0, 10.0] {
        if step * power >= value {
            return step * power;
        }
    }
    10.0 * power
}

/// What one setting says on its row: the value, and for a choice the
/// short explanation of what it means.
fn settings_value(app: &App, row: &liveside_client::settings::Row, selected: bool) -> String {
    match (&app.settings.editing, selected) {
        // A secret is typed blind: a terminal may well be on a stream.
        (Some(text), true) if matches!(row.value, Value::Secret { .. }) => {
            format!("{}_", "*".repeat(text.chars().count()))
        }
        // The field being typed into, with a cursor block.
        (Some(text), true) => format!("{text}_"),
        _ => match &row.value {
            Value::Switch(true) => app.text().on.to_owned(),
            Value::Switch(false) => app.text().off.to_owned(),
            Value::Number { value, .. } => value.to_string(),
            Value::Words(words) if words.is_empty() => app.text().settings_none.to_owned(),
            Value::Words(words) => words.join(", "),
            Value::Choice { options, index } => {
                let (name, meaning) = options[*index];
                format!("{name}  — {meaning}")
            }
            Value::Secret { typed: Some(_), .. } => app.text().key_new.to_owned(),
            Value::Secret { stored: true, .. } => app.text().key_set.to_owned(),
            Value::Secret { .. } => app.text().key_not_set.to_owned(),
            Value::Text(text) if text.is_empty() => row.when_empty(app.text()).to_owned(),
            Value::Text(text) => text.clone(),
            Value::Rules(rules) if rules.is_empty() => app.text().settings_none.to_owned(),
            Value::Rules(rules) => {
                let text = app.text();
                let (one, many) = match row.key {
                    "addressbook" => (text.addressbook_count_one, text.addressbook_count),
                    "custom_commands" => (text.commands_count_one, text.commands_count),
                    "hotkeys" => (text.hotkeys_count_one, text.hotkeys_count),
                    _ => (text.rules_count_one, text.rules_count),
                };
                counted(one, many, rules.len())
            }
        },
    }
}

/// The settings screen: one row per setting, the selected one highlighted,
/// with its explanation and the keys at the bottom.
fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let settings = &app.settings;
    if let Some(list) = &settings.rules {
        draw_rules(frame, app, list, area);
        return;
    }
    // Measured in columns, as the terminal draws them: a Czech label is
    // longer in bytes than on the screen, a Korean one shorter in letters.
    let label_width = settings
        .rows
        .iter()
        .map(|row| row.label.width())
        .max()
        .unwrap_or(0);
    let values: Vec<String> = settings
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| settings_value(app, row, i == settings.selected))
        .collect();
    // Wide enough for what it has to say, where the window allows it:
    // the explanation beside a choice is the point of showing it, and
    // cutting it in half says nothing.
    let needed = settings
        .rows
        .iter()
        .zip(&values)
        .map(|(row, value)| {
            let marker = if row.restart {
                app.text().settings_restart.width()
            } else {
                0
            };
            2 + label_width + 3 + value.width() + marker
        })
        .chain(std::iter::once(
            settings.rows[settings.selected].help.width() + 3,
        ))
        .max()
        .unwrap_or(76);
    let width = area
        .width
        .min(u16::try_from(needed).unwrap_or(u16::MAX).max(40));
    let headings = (0..settings.rows.len())
        .filter(|&i| settings.heading(i).is_some())
        .count();
    let height = area.height.min((settings.rows.len() + headings) as u16 + 8);
    let box_area = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    clear_for_box(frame, box_area);

    let mut lines: Vec<TextLine<'static>> = Vec::new();
    // Where the selected row is drawn, to keep it on screen when the
    // rows do not all fit.
    let mut selected_at = 0;
    for (i, row) in settings.rows.iter().enumerate() {
        if let Some(section) = settings.heading(i) {
            lines.push(TextLine::from(Span::styled(
                format!(" {}", section.title(app.text())),
                Style::new().fg(Color::Cyan).bold(),
            )));
        }
        let selected = i == settings.selected;
        if selected {
            selected_at = lines.len();
        }
        let shown = values[i].clone();
        let style = if selected {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new()
        };
        let marker = if row.restart {
            app.text().settings_restart
        } else {
            ""
        };
        // The box has a border on each side; what is left after the label
        // and the marker belongs to the value, which is the part that can
        // be arbitrarily long.
        let room = usize::from(width)
            .saturating_sub(2 + label_width + 3 + marker.width())
            .max(4);
        let shown = clip(&shown, room);
        lines.push(TextLine::from(vec![
            Span::styled(format!(" {}  ", pad(row.label, label_width)), style),
            Span::styled(shown, Style::new().fg(Color::Yellow)),
            Span::styled(marker, Style::new().fg(Color::DarkGray)),
        ]));
    }
    // Rows past the bottom of a short terminal scroll into view rather
    // than pushing the help and the keys off the screen.
    let footer = 4 + usize::from(settings.message.is_some());
    let room = usize::from(height).saturating_sub(2 + footer).max(1);
    if lines.len() > room {
        let first = (selected_at + 1).saturating_sub(room);
        lines = lines.into_iter().skip(first).take(room).collect();
    }
    lines.push(TextLine::from(""));
    lines.push(TextLine::from(Span::styled(
        clip(
            &format!(" {}", settings.rows[settings.selected].help),
            usize::from(width).saturating_sub(2),
        ),
        Style::new().fg(Color::DarkGray),
    )));
    let keys = if settings.editing.is_some() {
        app.text().settings_editing_keys
    } else {
        app.text().settings_keys
    };
    lines.push(TextLine::from(Span::styled(
        keys,
        Style::new().fg(Color::DarkGray),
    )));
    if let Some(message) = &settings.message {
        lines.push(TextLine::from(Span::styled(
            clip(&format!(" {message}"), usize::from(width).saturating_sub(2)),
            Style::new().fg(Color::Green),
        )));
    }

    // A long path would push the border apart, so the title is clipped too.
    let title = clip(
        &fill(
            app.text().settings_title,
            &[("path", &settings.path().display().to_string())],
        ),
        usize::from(width).saturating_sub(4),
    );
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title)
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        box_area,
    );
}

/// A list of rules being edited, in place of the settings: one rule a
/// line, the one being typed into with a cursor, and what is wrong with
/// it below.
fn draw_rules(
    frame: &mut Frame,
    app: &App,
    list: &liveside_client::settings::RuleList,
    area: Rect,
) {
    let settings = &app.settings;
    let row = &settings.rows[settings.selected];
    // Never wider than the terminal: the box is centred by subtracting.
    let width = area.width.min(110);
    let height = area.height.min(
        u16::try_from(list.lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(9),
    );
    let box_area = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    clear_for_box(frame, box_area);
    let inner = usize::from(width).saturating_sub(2);

    let mut lines: Vec<TextLine<'static>> = Vec::new();
    if list.lines.is_empty() {
        lines.push(TextLine::from(Span::styled(
            format!(
                " {}",
                match row.key {
                    "addressbook" => app.text().addressbook_empty,
                    "custom_commands" => app.text().commands_empty,
                    "hotkeys" => app.text().hotkeys_empty,
                    _ => app.text().rules_empty,
                }
            ),
            Style::new().fg(Color::DarkGray),
        )));
    }
    for (i, rule) in list.lines.iter().enumerate() {
        let selected = i == list.selected;
        let shown = match (&settings.editing, selected) {
            (Some(text), true) => format!("{text}_"),
            _ => rule.clone(),
        };
        let style = if selected {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new().fg(Color::Yellow)
        };
        let number = format!(" {:>2}. ", i + 1);
        // The end of a long rule is where it is being typed.
        let room = inner.saturating_sub(number.width()).max(4);
        let shown = if shown.width() > room && selected && settings.editing.is_some() {
            format!("…{}", tail(&shown, room - 1))
        } else {
            clip(&shown, room)
        };
        lines.push(TextLine::from(vec![
            Span::styled(number, Style::new().fg(Color::DarkGray)),
            Span::styled(shown, style),
        ]));
    }
    let footer = 4 + usize::from(list.error.is_some());
    let room = usize::from(height).saturating_sub(2 + footer).max(1);
    if lines.len() > room {
        let first = (list.selected + 1).saturating_sub(room);
        lines = lines.into_iter().skip(first).take(room).collect();
    }
    lines.push(TextLine::from(""));
    if let Some(error) = &list.error {
        lines.push(TextLine::from(Span::styled(
            clip(&format!(" {error}"), inner),
            Style::new().fg(Color::Red),
        )));
    }
    lines.push(TextLine::from(Span::styled(
        clip(&format!(" {}", row.help), inner),
        Style::new().fg(Color::DarkGray),
    )));
    let keys = if settings.editing.is_some() {
        app.text().settings_editing_keys
    } else {
        app.text().rules_keys
    };
    lines.push(TextLine::from(Span::styled(
        keys,
        Style::new().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(clip(&format!(" {} ", row.label), inner))
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        box_area,
    );
}

/// The tabs, the account and the room modes. `width` is the window, so the
/// tabs can be scrolled: with a dozen channels they no longer fit, and an
/// active tab cut in half is worse than one that is simply not shown.
/// A buffer's name. The status buffer has none of its own: it is a word in
/// the interface language, unlike a channel.
fn title_of(app: &App, buffer: &crate::app::Buffer) -> String {
    if buffer.channel.is_none() && buffer.title.is_empty() {
        app.text().status_buffer.to_owned()
    } else {
        buffer.title.clone()
    }
}

fn header_line(app: &App, width: u16) -> Paragraph<'_> {
    let labels: Vec<String> = app
        .buffers
        .iter()
        .enumerate()
        .map(|(i, buffer)| {
            // A dot before the name for a channel that is streaming. It
            // has to be a character rather than a colour: one style is
            // drawn per tab, and a colour of its own would mean splitting
            // every tab into two spans for one dot.
            // A wheel while the channel is being joined, a dot while it
            // is streaming: the first says the client is working, which
            // an empty tab bar for most of a minute does not.
            let mark = match buffer.joining.is_some() {
                true => {
                    let frame = crate::app::SPINNER[app.spinner % crate::app::SPINNER.len()];
                    format!("{frame} ")
                }
                false if buffer.live => "●".to_owned(),
                false => String::new(),
            };
            format!(" {}:{mark}{} ", i + 1, title_of(app, buffer))
        })
        .collect();

    // What the right-hand side needs; the tabs get the rest.
    let text = app.text();
    let state = match &app.connection {
        ConnectionState::Connected => text.connected.to_owned(),
        ConnectionState::Connecting => text.connecting.to_owned(),
        ConnectionState::Disconnected => text.offline.to_owned(),
        ConnectionState::Reconnecting { attempt, .. } => {
            fill(text.reconnecting, &[("attempt", &attempt.to_string())])
        }
    };
    let account = match &app.account {
        Some(account) => account.user.display_name.clone(),
        None => text.not_logged_in.to_owned(),
    };
    // A wheel before the account, for as long as an answer is owed:
    // opening a card or a list of people is a question to Twitch.
    let wheel = match app.working() {
        true => {
            let frame = crate::app::SPINNER[app.spinner % crate::app::SPINNER.len()];
            format!("  {frame}")
        }
        false => String::new(),
    };
    // How the stream on screen stands, when it is streaming.
    let stats = match (&app.buffer().stats, app.buffer().live) {
        (Some(stats), true) => format!(
            "  {}",
            liveside_client::lang::stream_stats(text, stats, liveside_proto::Timestamp::now())
        ),
        _ => String::new(),
    };
    let right = format!("{stats}{wheel}  [{account} | {state}]");
    let modes = mode_labels(&app.buffer().modes);
    let modes_text = if modes.is_empty() {
        String::new()
    } else {
        format!(" {}", modes.join(" "))
    };
    let demo = if !app.backend.is_empty() && app.backend != "twitch" {
        format!(" {} ", app.backend.to_uppercase())
    } else {
        String::new()
    };
    let reserved = right.width() + modes_text.width() + demo.width() + 1;
    let room = usize::from(width).saturating_sub(reserved).max(10);

    // Grow a window of tabs around the active one until it fills the room.
    let (first, last) = tab_window(&labels, app.active, room);
    let mut spans = Vec::new();
    if first > 0 {
        spans.push(Span::styled("‹", Style::new().fg(Color::DarkGray)));
    }
    for (i, label) in labels.iter().enumerate().take(last).skip(first) {
        let style = if i == app.active {
            Style::new().fg(Color::Black).bg(Color::Cyan).bold()
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(label.clone(), style));
    }
    if last < labels.len() {
        spans.push(Span::styled("›", Style::new().fg(Color::DarkGray)));
    }

    // Demo chat is invented locally and otherwise looks entirely real, so
    // it is labelled where the eye goes first.
    if !demo.is_empty() {
        spans.push(Span::styled(
            demo,
            Style::new().fg(Color::Black).bg(Color::Red).bold(),
        ));
    }
    spans.push(Span::styled(right, Style::new().fg(Color::DarkGray)));
    if !modes_text.is_empty() {
        spans.push(Span::styled(
            modes_text,
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    }
    Paragraph::new(TextLine::from(spans))
}

/// Which tabs to show in `room` columns, always including `active`. Returns
/// a half-open range; it grows from the active tab, forwards first, so the
/// following channels stay visible while reading one of the first.
fn tab_window(labels: &[String], active: usize, room: usize) -> (usize, usize) {
    if labels.is_empty() {
        return (0, 0);
    }
    let active = active.min(labels.len() - 1);
    let mut first = active;
    let mut last = active + 1;
    let mut used = labels[active].width();
    loop {
        let grew_back = last < labels.len() && used + labels[last].width() <= room;
        if grew_back {
            used += labels[last].width();
            last += 1;
        }
        let grew_front = first > 0 && used + labels[first - 1].width() <= room;
        if grew_front {
            first -= 1;
            used += labels[first].width();
        }
        if !grew_back && !grew_front {
            return (first, last);
        }
    }
}

/// Short labels for the restrictions in force, as Chatty shows them in its
/// title bar. Empty when the channel is unrestricted.
fn mode_labels(modes: &RoomModes) -> Vec<String> {
    let mut labels = Vec::new();
    if modes.subscribers_only {
        labels.push("sub".to_owned());
    }
    if modes.emote_only {
        labels.push("emote".to_owned());
    }
    if let Some(minutes) = modes.followers_only {
        labels.push(match minutes {
            0 => "follow".to_owned(),
            m if m % 1440 == 0 => format!("follow {}d", m / 1440),
            m if m % 60 == 0 => format!("follow {}h", m / 60),
            m => format!("follow {m}m"),
        });
    }
    if let Some(seconds) = modes.slow {
        labels.push(format!("slow {seconds}s"));
    }
    if modes.unique_chat {
        labels.push("unique".to_owned());
    }
    labels
}

/// Shortens text to `room` columns, ending in an ellipsis when it did not
/// fit. Counted in characters, which is close enough for settings text.
fn clip(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_owned();
    }
    // Room is in columns, and a Korean or Chinese letter takes two.
    let mut out = String::new();
    let mut used = 0;
    for letter in text.chars() {
        let wide = letter.width().unwrap_or(0);
        if used + wide > room.saturating_sub(1) {
            break;
        }
        used += wide;
        out.push(letter);
    }
    out.push('…');
    out
}

/// Clears `area` for a box drawn over the chat. A wide letter (Korean,
/// Chinese) just left of it would cover the box's left border: its second
/// half lies on the border's column, and the terminal is never told to
/// draw the border there. So it gives way to a space.
///
/// A strip of chat a column or two wide beside a box that nearly fills
/// the screen reads as a fault, not as the chat behind a dialog: the
/// settings showed the first digit of every time down their left side
/// (seen 2026-10-28). A strip that narrow is cleared with the box.
fn clear_for_box(frame: &mut Frame, area: Rect) {
    const NARROWEST_COLUMNS: u16 = 3;
    const NARROWEST_ROWS: u16 = 2;
    let screen = frame.area();
    let mut cleared = area;
    if area.x.saturating_sub(screen.x) < NARROWEST_COLUMNS
        || screen.right().saturating_sub(area.right()) < NARROWEST_COLUMNS
    {
        cleared.x = screen.x;
        cleared.width = screen.width;
    }
    if area.y.saturating_sub(screen.y) < NARROWEST_ROWS
        || screen.bottom().saturating_sub(area.bottom()) < NARROWEST_ROWS
    {
        cleared.y = screen.y;
        cleared.height = screen.height;
    }
    frame.render_widget(Clear, cleared);
    if cleared.x < area.x {
        return;
    }
    let Some(left) = area.x.checked_sub(1) else {
        return;
    };
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        if let Some(cell) = buffer.cell_mut((left, y))
            && cell.symbol().width() > 1
        {
            cell.set_symbol(" ");
        }
    }
}

/// The end of `text` that fits in `room` columns.
fn tail(text: &str, room: usize) -> String {
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for letter in text.chars().rev() {
        let wide = letter.width().unwrap_or(0);
        if used + wide > room {
            break;
        }
        used += wide;
        kept.push(letter);
    }
    kept.into_iter().rev().collect()
}

/// `text` filled out with spaces to `width` columns, as `{:width$}` would
/// if it counted columns rather than letters.
pub(crate) fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

/// How wide the user list may be: a fifth of the window, between 12 and
/// 22 columns, and nothing at all when that would squeeze the chat.
fn users_width(app: &App, total: u16) -> u16 {
    if !app.show_users || app.buffer().channel.is_none() {
        return 0;
    }
    let wanted = (total / 5).clamp(12, 22);
    if total.saturating_sub(wanted) < 30 {
        return 0;
    }
    wanted
}

/// The people who have talked in this channel, ranked then alphabetical.
fn draw_users(frame: &mut Frame, app: &App, area: Rect) {
    let chatters = app.chatters();
    // One column goes to the rule that separates the list from the chat.
    let width = area.width.saturating_sub(1) as usize;
    let inner = area.height.saturating_sub(1) as usize;
    let mut lines: Vec<TextLine<'static>> = vec![TextLine::from(Span::styled(
        format!(
            "{:width$}",
            counted(
                app.text().active_count_one,
                app.text().active_count,
                chatters.len()
            )
        ),
        Style::new().fg(Color::Black).bg(Color::DarkGray),
    ))];
    // A row is kept for "+N more" when the list does not fit.
    let room = if chatters.len() > inner {
        inner.saturating_sub(1)
    } else {
        inner
    };
    for chatter in chatters.iter().take(room) {
        let name: String = chatter.name.chars().take(width.saturating_sub(1)).collect();
        lines.push(TextLine::from(vec![
            Span::styled(chatter.symbol.to_string(), Style::new().fg(Color::Green)),
            Span::styled(name, Style::new().fg(Color::Gray)),
        ]));
    }
    if chatters.len() > room {
        lines.push(TextLine::from(Span::styled(
            fill(
                app.text().more_users,
                &[("count", &(chatters.len() - room).to_string())],
            ),
            Style::new().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::LEFT)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
}

/// Continuation rows line up under the message text, past the clock.
const INDENT: usize = 6;

fn draw_chat(frame: &mut Frame, app: &App, area: Rect) -> Vec<LinkArea> {
    let buffer = app.buffer();
    let width = area.width as usize;
    let height = area.height as usize;

    // Wrap from the newest line backwards, skipping what the user scrolled
    // past, and only as far as the view needs. Scrolling counts lines, so
    // the position does not shift when the window is resized.
    let mut rows: Vec<(TextLine<'static>, Vec<Link>)> = Vec::new();
    let mut more_above = false;
    let today = days::today();
    for (nth, line) in buffer.lines.iter().rev().skip(buffer.scroll).enumerate() {
        if rows.len() >= height {
            more_above = true;
            break;
        }
        let mut wrapped: Vec<(TextLine<'static>, Vec<Link>)> = render_rows(line, app, width)
            .into_iter()
            .enumerate()
            .flat_map(|(part, (row, links))| {
                // Which link of which line: what the pieces of a wrapped
                // link share.
                let links = links
                    .into_iter()
                    .enumerate()
                    .map(|(span, url)| {
                        url.map(|url| (format!("{nth}.{part}.{span}"), url.to_owned()))
                    })
                    .collect();
                wrap_linked(row, links, width, INDENT)
            })
            .collect();
        wrapped.reverse();
        rows.extend(wrapped);
        // Above the first line of a day, the date. It is not a line of
        // the channel, so scrolling does not count it.
        let at = buffer.lines.len() - 1 - buffer.scroll - nth;
        let previous = at
            .checked_sub(1)
            .and_then(|before| buffer.lines.get(before))
            .and_then(|before| days::day_of(before.time));
        if let Some(day) =
            days::day_of(line.time).and_then(|day| days::day_starts(previous, day, today))
        {
            let row = TextLine::from(Span::styled(
                format!("── {} ──", days::day_line(app.text(), day)),
                Style::new().fg(Color::DarkGray),
            ));
            rows.push((row, vec![None]));
        }
    }
    rows.reverse();

    // A tall line at the top may overflow the view; keep its tail.
    let start = rows.len().saturating_sub(height);
    let visible: Vec<TextLine<'static>> =
        rows[start..].iter().map(|(row, _)| row.clone()).collect();
    frame.render_widget(Paragraph::new(visible), area);

    let mut links = Vec::new();
    for (y, (row, row_links)) in (area.y..).zip(&rows[start..]) {
        let mut x = area.x;
        for (span, link) in row.spans.iter().zip(row_links) {
            let span_width = span.width() as u16;
            if let Some((id, url)) = link {
                links.push(LinkArea {
                    x,
                    y,
                    width: span_width.min(area.right().saturating_sub(x)),
                    url: url.clone(),
                    id: id.clone(),
                });
            }
            x = x.saturating_add(span_width);
        }
    }

    if buffer.scroll > 0 && (more_above || start > 0 || !rows.is_empty()) {
        let marker = Span::styled(
            fill(
                app.text().scrolled_back,
                &[("lines", &buffer.scroll.to_string())],
            ),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        );
        let width = marker.width() as u16;
        let x = area.right().saturating_sub(width.min(area.width));
        let rect = Rect::new(x, area.bottom().saturating_sub(1), width.min(area.width), 1);
        frame.render_widget(Paragraph::new(TextLine::from(marker)), rect);
        // The marker covers the end of the last row; a link under it is
        // cut where the marker starts.
        for link in &mut links {
            if link.y == rect.y && link.x + link.width > rect.x {
                link.width = rect.x.saturating_sub(link.x);
            }
        }
        links.retain(|link| link.width > 0);
    }
    links
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    // Searching takes the line over: the query, and how it went.
    if let Some(search) = &app.search {
        let prompt = format!("[{}] ", app.text().search_prompt);
        let count = match search.at {
            Some((at, total)) => fill(
                app.text().search_count,
                &[("at", &at.to_string()), ("total", &total.to_string())],
            ),
            None if search.query.is_empty() => String::new(),
            None => app.text().search_none.to_owned(),
        };
        let line = TextLine::from(vec![
            Span::styled(prompt.clone(), Style::new().fg(Color::Yellow)),
            Span::raw(&search.query),
            Span::raw("  "),
            Span::styled(count, Style::new().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        let x = area.x + (prompt.width() + search.query.width()) as u16;
        frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
        return;
    }
    let title = title_of(app, app.buffer());
    // Answering someone must be visible while typing, or the reply goes
    // out attached to a message the user has forgotten about.
    let prompt = match &app.buffer().replying_to {
        Some(target) => format!("[{title}] ↳{} ", target.name),
        None => format!("[{title}] "),
    };
    let line = TextLine::from(vec![
        Span::styled(prompt.clone(), Style::new().fg(Color::Cyan)),
        Span::raw(&app.input.text),
    ]);
    frame.render_widget(Paragraph::new(line), area);

    let before_cursor = &app.input.text[..app.input.cursor];
    let x = area.x + (prompt.width() + before_cursor.width()) as u16;
    frame.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1)), area.y));
}

/// Which link a span belongs to, if any: the link's id, and its address.
type Link = Option<(String, String)>;

/// Wraps one rendered line into terminal rows, breaking on spaces and
/// keeping each span's style. Rows after the first are indented.
#[cfg(test)]
fn wrap(line: TextLine<'_>, width: usize, indent: usize) -> Vec<TextLine<'static>> {
    let links = vec![None; line.spans.len()];
    wrap_linked(line, links, width, indent)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// The same, keeping beside every span of every row the link it is part
/// of, so a link still knows where it is after being broken in two.
fn wrap_linked(
    line: TextLine<'_>,
    links: Vec<Link>,
    width: usize,
    indent: usize,
) -> Vec<(TextLine<'static>, Vec<Link>)> {
    if width == 0 {
        return Vec::new();
    }
    let indent = if indent < width { indent } else { 0 };
    let mut rows: Vec<(TextLine<'static>, Vec<Link>)> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_links: Vec<Link> = Vec::new();
    let mut used = 0;

    let mut start_row =
        |row: &mut Vec<Span<'static>>, row_links: &mut Vec<Link>, used: &mut usize| {
            rows.push((
                TextLine::from(std::mem::take(row)),
                std::mem::take(row_links),
            ));
            *used = indent;
            if indent > 0 {
                row.push(Span::raw(" ".repeat(indent)));
                row_links.push(None);
            }
        };

    for (span, link) in line.spans.into_iter().zip(links) {
        for word in split_keeping_spaces(&span.content) {
            let mut word = word;
            loop {
                let word_width = word.width();
                if used + word_width <= width {
                    row.push(Span::styled(word.to_owned(), span.style));
                    row_links.push(link.clone());
                    used += word_width;
                    break;
                }
                if word_width <= width.saturating_sub(indent) {
                    // Fits on a fresh row: wrap, dropping a leading space.
                    start_row(&mut row, &mut row_links, &mut used);
                    word = word.trim_start_matches(' ');
                    if word.is_empty() {
                        break;
                    }
                    continue;
                }
                // Longer than any row: fill this one and carry the rest.
                let take = take_columns(word, width - used);
                if take.is_empty() {
                    start_row(&mut row, &mut row_links, &mut used);
                    continue;
                }
                row.push(Span::styled(take.to_owned(), span.style));
                row_links.push(link.clone());
                used += take.width();
                word = &word[take.len()..];
            }
        }
    }
    rows.push((TextLine::from(row), row_links));
    rows
}

/// Splits into words, each keeping the spaces that preceded it.
fn split_keeping_spaces(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_spaces = text.starts_with(' ');
    for (i, c) in text.char_indices() {
        if c == ' ' && !in_spaces {
            if i > start {
                parts.push(&text[start..i]);
            }
            start = i;
            in_spaces = true;
        } else if c != ' ' {
            in_spaces = false;
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

/// The longest prefix of `text` that fits in `columns` display columns.
fn take_columns(text: &str, columns: usize) -> &str {
    let mut used = 0;
    for (i, c) in text.char_indices() {
        let w = c.to_string().width();
        if used + w > columns {
            return &text[..i];
        }
        used += w;
    }
    text
}

/// The rows one line occupies before wrapping: a reply shows what it
/// answers above itself, the way every graphical client does, because a
/// bare "yes" in a fast channel says nothing on its own.
fn render_rows<'a>(
    line: &'a Line,
    app: &App,
    width: usize,
) -> Vec<(TextLine<'a>, Vec<Option<&'a str>>)> {
    let mut rows = Vec::new();
    if let LineContent::Message(message) = &line.content
        && let Some(reply) = &message.reply
    {
        let row = reply_row(reply, width);
        let links = vec![None; row.spans.len()];
        rows.push((row, links));
    }
    let (mut row, mut links) = render_line(line, app.text(), &app.link_reports);
    if let Some(search) = app
        .search
        .as_ref()
        .filter(|search| !search.query.is_empty())
    {
        (row, links) = mark_found(row, links, &search.query);
    }
    // The selected message is what Ctrl-R answers. Both sides being
    // nothing is not a match: a notice carries no message id, and
    // comparing the two `None`s drew every notice as if it were selected.
    if let (Some(selected), Some(id)) = (app.buffer().selected.as_ref(), message_id(line))
        && selected == id
    {
        for span in &mut row.spans {
            span.style = span.style.add_modifier(Modifier::REVERSED);
        }
    }
    rows.push((row, links));
    rows
}

/// Puts what a search found on a yellow ground, splitting spans where a
/// match starts and ends; each piece keeps its span's link.
fn mark_found<'a>(
    row: TextLine<'a>,
    links: Vec<Option<&'a str>>,
    query: &str,
) -> (TextLine<'a>, Vec<Option<&'a str>>) {
    let mut spans = Vec::new();
    let mut marked_links = Vec::new();
    for (span, link) in row.spans.into_iter().zip(links) {
        let found = crate::app::find_ignoring_case(&span.content, query);
        if found.is_empty() {
            spans.push(span);
            marked_links.push(link);
            continue;
        }
        let text = span.content.to_string();
        let mut from = 0;
        for range in found {
            if range.start > from {
                spans.push(Span::styled(text[from..range.start].to_owned(), span.style));
                marked_links.push(link);
            }
            spans.push(Span::styled(
                text[range.clone()].to_owned(),
                span.style.fg(Color::Black).bg(Color::Yellow),
            ));
            marked_links.push(link);
            from = range.end;
        }
        if from < text.len() {
            spans.push(Span::styled(text[from..].to_owned(), span.style));
            marked_links.push(link);
        }
    }
    (TextLine::from(spans), marked_links)
}

fn message_id(line: &Line) -> Option<&liveside_proto::MessageId> {
    match &line.content {
        LineContent::Message(message) => Some(&message.id),
        _ => None,
    }
}

/// `↳ Name: what they said`, shortened to one row.
fn reply_row(reply: &ReplyTo, width: usize) -> TextLine<'static> {
    let room = width.saturating_sub(INDENT + reply.parent_user.display_name.len() + 6);
    let mut text: String = reply.parent_text.chars().take(room.max(8)).collect();
    if text.chars().count() < reply.parent_text.chars().count() {
        text.push('…');
    }
    TextLine::from(vec![
        Span::styled(
            format!("{:INDENT$}↳ {}: ", "", reply.parent_user.display_name),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(text, Style::new().fg(Color::DarkGray)),
    ])
}

fn render_line<'a>(
    line: &'a Line,
    strings: &'static liveside_client::lang::Strings,
    reports: &Reports,
) -> (TextLine<'a>, Vec<Option<&'a str>>) {
    let mut spans = vec![Span::styled(
        format!("{} ", clock(line.time)),
        Style::new().fg(Color::DarkGray),
    )];
    // Beside every span, the address when it is a link.
    let mut links: Vec<Option<&'a str>> = vec![None];
    match &line.content {
        LineContent::Message(message) => {
            for (span, link) in message_spans(message, &line.flags, strings, reports) {
                spans.push(span);
                links.push(link);
            }
        }
        LineContent::UserNotice {
            system_text,
            message,
        } => {
            spans.push(Span::styled(
                format!("* {system_text}"),
                Style::new().fg(Color::Magenta),
            ));
            links.push(None);
            if let Some(message) = message {
                spans.push(Span::raw(" "));
                links.push(None);
                for (span, link) in
                    fragment_spans(&message.fragments, Style::new(), strings, reports)
                {
                    spans.push(span);
                    links.push(link);
                }
            }
        }
        LineContent::Notice {
            level,
            text,
            code,
            args,
        } => {
            let color = match level {
                NoticeLevel::Info => Color::DarkGray,
                NoticeLevel::Warning => Color::Yellow,
                NoticeLevel::Error => Color::Red,
            };
            // A notice from the engine is said in the user's language when
            // this version knows its code.
            let said = liveside_client::lang::from_engine(strings, code, args, text);
            spans.push(Span::styled(format!("-- {said}"), Style::new().fg(color)));
            links.push(None);
        }
    }
    // An info line a rule lit up, all of it but the time; a message has
    // its text lit up where it is drawn.
    if line.flags.highlighted && !matches!(line.content, LineContent::Message(_)) {
        let lit = highlight_style(&line.flags);
        for span in spans.iter_mut().skip(1) {
            span.style = span.style.patch(lit);
        }
    }
    if line.flags.deleted {
        for span in &mut spans {
            // Struck through and faded, but faded by colour rather than
            // by the terminal's own dim attribute: dim over the grey of a
            // timestamp takes the time off the screen entirely.
            span.style = span.style.add_modifier(Modifier::CROSSED_OUT);
            span.style.fg = Some(fade(span.style.fg.unwrap_or(Color::Gray)));
        }
        // A message a moderator took down keeps its text on screen, but
        // not a link to click: taking it down was often about the link.
        links.iter_mut().for_each(|link| *link = None);
    } else if line.flags.historical {
        for span in &mut spans {
            span.style.fg = Some(fade(span.style.fg.unwrap_or(Color::Gray)));
        }
    }
    (TextLine::from(spans), links)
}

fn message_spans<'a>(
    message: &'a ChatMessage,
    flags: &LineFlags,
    strings: &'static liveside_client::lang::Strings,
    reports: &Reports,
) -> Vec<(Span<'a>, Option<&'a str>)> {
    let mut spans = Vec::new();
    // Why Twitch does not trust them, on the message itself: a list
    // somewhere else would be read long after this scrolled past.
    if let Some(suspicion) = &flags.suspicion {
        spans.push(Span::styled(
            format!(
                "[{}] ",
                liveside_client::lang::suspicion(strings, suspicion)
            ),
            // The light red on a dark background: Terminal.app's darker
            // palette made the plain one hard to read there. A light
            // background gets the plain red back (`on_light`).
            Style::new().fg(Color::LightRed).bold(),
        ));
    }
    // In a Shared Chat session the other channels' messages arrive here
    // as if they were ours. Saying which channel one came from is the
    // whole difference between a conversation and a crowd.
    if let Some(source) = &message.shared_source {
        spans.push(Span::styled(
            format!("#{} ", source.channel.login),
            Style::new().fg(Color::DarkGray),
        ));
    }
    let badges = badge_symbols(&message.badges);
    if !badges.is_empty() {
        spans.push(Span::styled(badges, Style::new().fg(Color::Green)));
    }
    let mut name_style = Style::new()
        .bold()
        .fg(name_color(&message.sender, message.color));
    if flags.own {
        name_style = name_style.underlined();
    }
    spans.push(Span::styled(display_name(&message.sender), name_style));
    // A /me reads as what they do: no colon, and in their colour.
    let action = message.kind == liveside_proto::model::MessageKind::Action;
    spans.push(Span::raw(if action { " " } else { ": " }));

    let text_style = if flags.highlighted {
        highlight_style(flags)
    } else if action {
        Style::new().fg(name_color(&message.sender, message.color))
    } else {
        Style::new()
    };
    let mut spans: Vec<(Span<'a>, Option<&'a str>)> =
        spans.into_iter().map(|span| (span, None)).collect();
    spans.extend(fragment_spans(
        &message.fragments,
        text_style,
        strings,
        reports,
    ));
    // How many times they have now said it, after what they said.
    if flags.repeated > 0 {
        spans.push((
            Span::styled(
                format!(" (×{})", flags.repeated),
                Style::new().fg(Color::DarkGray),
            ),
            None,
        ));
    }
    spans
}

/// What is known about links, by address and by who found it out.
type Reports = std::collections::HashMap<
    (String, liveside_proto::model::LinkSource),
    liveside_proto::model::LinkState,
>;

fn fragment_spans<'a>(
    fragments: &'a [Fragment],
    base: Style,
    strings: &'static liveside_client::lang::Strings,
    reports: &Reports,
) -> Vec<(Span<'a>, Option<&'a str>)> {
    fragments
        .iter()
        .flat_map(|fragment| match fragment {
            // What VirusTotal said goes right after the link, so it is
            // read with it.
            Fragment::Link { text, url } => {
                use liveside_client::lang::Verdict;
                use liveside_proto::model::LinkSource;
                let mut spans = vec![(
                    Span::styled(text.as_str(), base.underlined()),
                    Some(url.as_str()),
                )];
                for source in [LinkSource::VirusTotal, LinkSource::Redirects] {
                    let Some(state) = reports.get(&(url.clone(), source)) else {
                        continue;
                    };
                    let (said, verdict) =
                        liveside_client::lang::link_report(strings, source, state);
                    let style = match verdict {
                        Verdict::Waiting => Style::new().fg(Color::DarkGray),
                        Verdict::Clean => Style::new().fg(Color::Green),
                        Verdict::Dangerous => Style::new().fg(Color::Red).bold(),
                        Verdict::Unknown => Style::new().fg(Color::Yellow),
                        Verdict::Info => Style::new().fg(Color::Cyan),
                    };
                    spans.push((Span::styled(format!(" [{said}]"), style), None));
                }
                spans
            }
            other => vec![(
                match other {
                    Fragment::Text { text } => Span::styled(text.as_str(), base),
                    // No images in a terminal: emotes are their codes, marked by colour.
                    Fragment::Emote { emote } => {
                        Span::styled(emote.code.as_str(), base.fg(Color::LightMagenta))
                    }
                    Fragment::Mention { text, .. } => Span::styled(text.as_str(), base.bold()),
                    Fragment::Cheermote { text, .. } => {
                        Span::styled(text.as_str(), base.fg(Color::LightCyan))
                    }
                    Fragment::Gif { text, .. } => {
                        Span::styled(text.as_str(), base.fg(Color::LightMagenta))
                    }
                    Fragment::Link { .. } => unreachable!("handled above"),
                },
                None,
            )],
        })
        .collect()
}

/// Display name, falling back to the login when it is not just a different
/// spelling (e.g. localized names).
fn display_name(user: &User) -> &str {
    if user.display_name.eq_ignore_ascii_case(user.login.as_str()) {
        &user.display_name
    } else {
        user.login.as_str()
    }
}

/// Chatty-style symbols, so badges cost one column each.
fn badge_symbols(badges: &[Badge]) -> String {
    let mut out = String::new();
    for badge in badges {
        out.push(match badge.set_id.as_str() {
            "broadcaster" => '~',
            "moderator" => '@',
            "vip" => '!',
            "subscriber" | "founder" => '%',
            "premium" | "turbo" => '+',
            "bits" => '$',
            "staff" | "admin" => '&',
            _ => continue,
        });
    }
    if !out.is_empty() {
        out.push(' ');
    }
    out
}

/// How highlighted text looks: black on yellow, or the colours its rule
/// asked for.
fn highlight_style(flags: &LineFlags) -> Style {
    Style::new()
        .fg(flags.highlight_color.map_or(Color::Black, color))
        .bg(flags.highlight_background.map_or(Color::Yellow, color))
}

fn color(rgb: Rgb) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// Relative luminance (ITU-R BT.709), 0.0 black to 1.0 white.
fn luminance(rgb: Rgb) -> f32 {
    let (r, g, b) = (
        f32::from(rgb.r) / 255.0,
        f32::from(rgb.g) / 255.0,
        f32::from(rgb.b) / 255.0,
    );
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Twitch lets users pick names as dark as pure blue, which is unreadable on
/// a dark terminal, and as pale as light yellow, which is unreadable on a
/// light one. Colours too close to the background are blended away from
/// it until they pass, keeping the hue recognizable.
fn readable(rgb: Rgb) -> Rgb {
    readable_on(rgb, light_background())
}

fn readable_on(rgb: Rgb, light: bool) -> Rgb {
    if light {
        darken_to(rgb, 0.45)
    } else {
        lift_to(rgb, 0.4)
    }
}

/// Blends towards black until the luminance is down to `target`, which
/// keeps the hue while making the colour visible on a light background.
fn darken_to(rgb: Rgb, target: f32) -> Rgb {
    let lum = luminance(rgb);
    if lum <= target {
        return rgb;
    }
    // Blending with black lowers luminance linearly, so this is exact.
    let keep = target / lum;
    let darken = |c: u8| (f32::from(c) * keep).round().clamp(0.0, 255.0) as u8;
    Rgb {
        r: darken(rgb.r),
        g: darken(rgb.g),
        b: darken(rgb.b),
    }
}

/// Blends towards white until the luminance reaches `target`, which keeps
/// the hue while making the colour visible on a dark background.
fn lift_to(rgb: Rgb, target: f32) -> Rgb {
    let lum = luminance(rgb);
    if lum >= target {
        return rgb;
    }
    // Blending with white raises luminance linearly, so this is exact.
    let t = (target - lum) / (1.0 - lum);
    let lift = |c: u8| {
        let c = f32::from(c);
        (c + (255.0 - c) * t).round().clamp(0.0, 255.0) as u8
    };
    Rgb {
        r: lift(rgb.r),
        g: lift(rgb.g),
        b: lift(rgb.b),
    }
}

/// Muted version of a colour, for the history loaded on join.
///
/// The terminal's own "dim" attribute is not used: Alacritty and others
/// apply it only to the sixteen palette colours, so a true-colour nick
/// stayed bright while the grey timestamp went invisible.
fn fade(color: Color) -> Color {
    fade_on(color, light_background())
}

fn fade_on(color: Color, light: bool) -> Color {
    let Some(rgb) = as_rgb(color) else {
        return color;
    };
    // On a light background, fading goes towards white instead ...
    if light {
        let towards_white = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * 0.5) as u8;
        let paled = Rgb {
            r: towards_white(rgb.r),
            g: towards_white(rgb.g),
            b: towards_white(rgb.b),
        };
        // ... and stops before it is gone.
        let Rgb { r, g, b } = darken_to(paled, 0.7);
        return Color::Rgb(r, g, b);
    }
    let dimmed = Rgb {
        r: (f32::from(rgb.r) * 0.55) as u8,
        g: (f32::from(rgb.g) * 0.55) as u8,
        b: (f32::from(rgb.b) * 0.55) as u8,
    };
    // ... but not so far that it disappears into the background: most
    // dark themes are grey rather than black (Ghostty's is about 0.17),
    // and 0.18 left the time of a restored line a shade off it.
    let Rgb { r, g, b } = lift_to(dimmed, 0.33);
    Color::Rgb(r, g, b)
}

/// The palette colours this frontend uses, as RGB. Terminals theme these,
/// so the values are only approximations, used for fading.
fn as_rgb(color: Color) -> Option<Rgb> {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::LightRed => (241, 76, 76),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::White => (255, 255, 255),
        _ => return None,
    };
    Some(Rgb { r, g, b })
}

/// The user's colour, or a stable one derived from the login so names stay
/// distinguishable even before Twitch tells us a colour.
fn name_color(user: &User, set: Option<Rgb>) -> Color {
    if let Some(rgb) = set {
        return color(readable(rgb));
    }
    const FALLBACK: [Color; 8] = [
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::LightRed,
        Color::LightGreen,
    ];
    let sum: u32 = user.login.as_str().bytes().map(u32::from).sum();
    // Yellow is the palette colour most terminals leave too pale for a
    // light background; the light ones become their deeper siblings when
    // drawn.
    let picked = FALLBACK[sum as usize % FALLBACK.len()];
    if light_background() && picked == Color::Yellow {
        return Color::Magenta;
    }
    picked
}

/// Local time `HH:MM`.
pub(crate) fn clock(time: Timestamp) -> String {
    let Ok(stamp) = JiffTimestamp::from_millisecond(time.0) else {
        return "--:--".into();
    };
    let zoned = stamp.to_zoned(TimeZone::system());
    format!("{:02}:{:02}", zoned.hour(), zoned.minute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::{Channel, EmoteProvider, EmoteRef, MessageKind, RoomModes};
    use liveside_proto::{ChannelState, ConnectionState, Event, ServerMessage, Snapshot};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn what_virustotal_said_follows_the_link() {
        use liveside_proto::model::LinkState;
        let fragments = vec![
            Fragment::Text {
                text: "see ".into(),
            },
            Fragment::Link {
                text: "http://github.com/".into(),
                url: "http://github.com/".into(),
            },
            Fragment::Text {
                text: " now".into(),
            },
        ];
        let strings = liveside_client::lang::Language::English.strings();
        let mut reports = Reports::new();
        let text = |reports: &Reports| -> String {
            fragment_spans(&fragments, Style::new(), strings, reports)
                .iter()
                .map(|(span, _)| span.content.as_ref())
                .collect()
        };
        assert_eq!(text(&reports), "see http://github.com/ now");
        reports.insert(
            (
                "http://github.com/".into(),
                liveside_proto::model::LinkSource::VirusTotal,
            ),
            LinkState::Found {
                malicious: 2,
                suspicious: 0,
                engines: 93,
                analysed: None,
                final_url: Some("https://github.com/".into()),
                redirects: Vec::new(),
            },
        );
        assert_eq!(
            text(&reports),
            "see http://github.com/ [VirusTotal: 2 malicious, 0 suspicious of 93 · \
             ends at https://github.com/] now"
        );
        // And where it leads, after VirusTotal's verdict.
        reports.insert(
            (
                "http://github.com/".into(),
                liveside_proto::model::LinkSource::Redirects,
            ),
            LinkState::Leads {
                final_url: Some("https://github.com/".into()),
                redirects: vec!["http://github.com/".into(), "https://github.com/".into()],
                status: 200,
            },
        );
        assert!(
            text(&reports).ends_with(
                "github.com/] [leads to https://github.com/ (redirects: 1, HTTP only)] now"
            ),
            "{}",
            text(&reports)
        );
    }

    fn message_line(login: &str, text: &str, flags: LineFlags) -> Line {
        // One id per name, so that tests about people are about people.
        let user = User {
            id: login.into(),
            login: login.into(),
            display_name: login.into(),
        };
        Line {
            time: Timestamp(1_790_000_000_000),
            content: LineContent::Message(ChatMessage {
                id: "m1".into(),
                sender: user,
                color: None,
                badges: vec![Badge {
                    set_id: "moderator".into(),
                    id: "1".into(),
                    info: String::new(),
                    image: String::new(),
                    title: String::new(),
                }],
                fragments: vec![
                    Fragment::Text {
                        text: format!("{text} "),
                    },
                    Fragment::Emote {
                        emote: EmoteRef {
                            provider: EmoteProvider::Twitch,
                            id: "25".into(),
                            code: "Kappa".into(),
                            animated: false,
                            zero_width: false,
                        },
                    },
                ],
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags,
        }
    }

    fn app_with_channel() -> App {
        let mut app = App::default();
        app.apply(ServerMessage::Welcome {
            protocol: 1,
            server: "test".into(),
        });
        app.apply(ServerMessage::Snapshot {
            seq: liveside_proto::Seq(1),
            snapshot: Snapshot {
                account: None,
                backend: "twitch".into(),
                connection: ConnectionState::Connected,
                channels: vec![ChannelState {
                    channel: Channel {
                        id: "1001".into(),
                        login: "somechannel".into(),
                        display_name: "SomeChannel".into(),
                    },
                    modes: RoomModes::default(),
                    lines: vec![message_line("alice", "hello there", LineFlags::default())],
                    chatters: Vec::new(),
                    live: false,
                    held: Vec::new(),
                    unban_requests: Vec::new(),
                    poll: None,
                    prediction: None,
                    redemptions: Vec::new(),
                    shield: false,
                    stats: None,
                    viewers: Default::default(),
                }],
                data_flows: Vec::new(),
                whispers: Vec::new(),
                link_reports: Vec::new(),
                highlighted: Vec::new(),
                ignored: Vec::new(),
                bots: Vec::new(),
            },
        });
        app.active = 1;
        app
    }

    /// Renders and returns the screen as plain text lines.
    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn rows(text: &str, width: usize, indent: usize) -> Vec<String> {
        let line = TextLine::from(vec![Span::raw(text)]);
        wrap(line, width, indent)
            .iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrapping_breaks_on_spaces_and_indents() {
        assert_eq!(rows("hello world", 20, 0), ["hello world"]);
        assert_eq!(rows("hello world", 7, 0), ["hello", "world"]);
        assert_eq!(
            rows("hello world again", 8, 2),
            ["hello", "  world", "  again"]
        );
    }

    #[test]
    fn wrapping_splits_words_longer_than_the_width() {
        // A word no row can hold (a long URL) fills the current row and
        // continues, rather than leaving a ragged gap.
        assert_eq!(rows("ab abcdefghij", 5, 0), ["ab ab", "cdefg", "hij"]);
        // Wide characters count as two columns and are never cut in half.
        assert_eq!(rows("日本語です", 4, 0), ["日本", "語で", "す"]);
    }

    #[test]
    fn pale_colours_are_darkened_on_a_light_background() {
        let pale = Rgb {
            r: 255,
            g: 255,
            b: 160,
        };
        let fixed = readable_on(pale, true);
        assert!(luminance(fixed) <= 0.46, "{fixed:?}");
        // Still yellow: red and green ahead of blue.
        assert!(fixed.r > fixed.b && fixed.g > fixed.b, "{fixed:?}");
        // A dark name is already readable there, and left alone.
        let navy = Rgb { r: 0, g: 0, b: 90 };
        assert_eq!(readable_on(navy, true), navy);
        // The same pale yellow is fine on a dark background.
        assert_eq!(readable_on(pale, false), pale);
    }

    #[test]
    fn history_fades_towards_the_background() {
        let orange = Color::Rgb(200, 120, 0);
        let lum = |c: Color| luminance(as_rgb(c).unwrap());
        assert!(lum(fade_on(orange, false)) < lum(orange));
        assert!(lum(fade_on(orange, true)) > lum(orange));
        // ... but never all the way to white.
        assert!(lum(fade_on(Color::Rgb(250, 250, 250), true)) <= 0.71);
        // Nor, on a dark one, into a background that is grey rather
        // than black: the time of a restored line stays readable.
        assert!(lum(fade_on(Color::DarkGray, false)) >= 0.3);
    }

    #[test]
    fn pale_palette_colours_deepen_on_a_light_background() {
        assert_eq!(on_light(Color::Gray), Color::DarkGray);
        assert_eq!(on_light(Color::White), Color::Black);
        assert_eq!(on_light(Color::LightCyan), Color::Cyan);
        assert_eq!(on_light(Color::Red), Color::Red);
        assert_eq!(on_light(Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn dark_colours_are_lifted_to_stay_readable() {
        let dark_blue = Rgb::parse_hex("#0000ff").unwrap();
        let fixed = readable(dark_blue);
        assert!(luminance(fixed) >= 0.39, "{fixed} is still too dark");
        assert!(
            fixed.b > fixed.r && fixed.b > fixed.g,
            "{fixed} lost its hue"
        );

        // Bright colours are left exactly as the user chose them.
        let bright = Rgb::parse_hex("#00ff7f").unwrap();
        assert_eq!(readable(bright), bright);

        // Even black becomes visible.
        let black = Rgb { r: 0, g: 0, b: 0 };
        assert!(luminance(readable(black)) >= 0.39);
    }

    #[test]
    fn history_is_faded_but_still_visible() {
        // A bright nick colour is muted...
        let faded = fade(Color::Rgb(255, 0, 0));
        match faded {
            Color::Rgb(r, g, b) => {
                assert!(r < 255 && r > 60, "{faded:?} should be muted, not gone");
                assert!(r > g && r > b, "{faded:?} lost its hue");
            }
            other => panic!("expected an rgb colour, got {other:?}"),
        }
        // ... and the timestamp grey stays above the floor, which the
        // terminal's own dim attribute did not manage.
        let clock = fade(Color::DarkGray);
        let rgb = as_rgb(clock).unwrap();
        assert!(luminance(rgb) >= 0.17, "{clock:?} is invisible");
    }

    #[test]
    fn the_settings_box_grows_to_fit_what_it_says() {
        // The explanation beside a choice is the reason for showing the
        // choice at all, and a box of a fixed width cut it in half.
        use liveside_client::settings::Settings;
        let mut app = app_with_channel();
        app.settings = Settings::new(
            liveside_client::Config::default(),
            "/home/someone/.config/liveside/config.toml".into(),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        app.settings.open();

        let wide = screen(&app, 120, 24).join("\n");
        assert!(!wide.contains('…'), "clipped in a wide window:\n{wide}");

        // In a narrow one it still has to fit, and clipping is right.
        let narrow = screen(&app, 44, 24).join("\n");
        assert!(
            narrow.lines().all(|row| row.chars().count() <= 44),
            "wider than the window:\n{narrow}"
        );
    }

    #[test]
    fn a_suspicious_message_says_why_on_the_message() {
        // The reason belongs where the message is: a list somewhere else
        // is read long after this has scrolled past.
        let mut app = app_with_channel();
        let mut line = message_line("alice", "hello", LineFlags::default());
        line.flags.suspicion = Some(Box::new(liveside_proto::model::Suspicion {
            status: liveside_proto::model::SuspicionStatus::Monitored,
            reasons: vec!["ban_evader".to_owned()],
            evasion: "likely".to_owned(),
            shared_bans: 0,
        }));
        app.buffers[1].push(line);

        let view = screen(&app, 80, 8).join("\n");
        assert!(view.contains("under watch"), "{view}");
        assert!(view.contains("ban evader"), "{view}");
        assert!(view.contains("hello"), "the message itself is still there");
    }

    #[test]
    fn the_viewer_graph_is_drawn_in_braille_with_its_changes() {
        let mut app = app_with_channel();
        let started = Timestamp(0);
        for (minute, viewers) in [(0, 1000), (1, 1500), (2, 1100), (3, 1300)] {
            app.buffers[1].viewers.record(
                Timestamp(1_800_000_000_000 + minute * 60_000),
                &liveside_proto::model::StreamStats { viewers, started },
            );
        }
        app.buffers[1]
            .viewers
            .mark(Timestamp(1_800_000_090_000), "Fixing the graph", "Software");
        app.graph = app.buffers[1].channel.clone();
        let view = screen(&app, 100, 30).join("\n");
        assert!(view.contains("Now 1 300 · most 1 500 at "), "{view}");
        assert!(view.contains("1. "), "{view}");
        assert!(view.contains("Fixing the graph — Software"), "{view}");
        assert!(
            view.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)),
            "braille dots: {view}"
        );
        assert!(view.contains("2 000"), "the axis is round: {view}");
    }

    #[test]
    fn a_me_message_reads_as_what_they_do() {
        let mut app = app_with_channel();
        let mut line = message_line("bot", "waves", LineFlags::default());
        if let LineContent::Message(message) = &mut line.content {
            message.kind = liveside_proto::model::MessageKind::Action;
        }
        app.buffers[1].push(line);
        let view = screen(&app, 80, 8).join("\n");
        assert!(view.contains("bot waves"), "{view}");
        assert!(!view.contains("bot: waves"), "{view}");
    }

    #[test]
    fn a_repeated_message_says_how_many_times() {
        let mut app = app_with_channel();
        let flags = LineFlags {
            repeated: 3,
            ..LineFlags::default()
        };
        app.buffers[1].push(message_line("bot", "buy followers", flags));
        app.buffers[1].push(message_line("alice", "hi", LineFlags::default()));
        let view = screen(&app, 80, 8).join("\n");
        assert!(view.contains("bot: buy followers Kappa (×3)"), "{view}");
        assert!(!view.contains("hi (×"), "{view}");
    }

    #[test]
    fn a_deleted_message_keeps_its_time_readable() {
        // Struck through and faded, but the terminal's own dim attribute
        // over a grey timestamp leaves nothing on the screen at all.
        let mut app = app_with_channel();
        let mut line = message_line("alice", "oops", LineFlags::default());
        line.flags.deleted = true;
        app.buffers[1].push(line);

        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, &app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row = (0..buffer.area.height)
            .find(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, *y)].symbol())
                    .collect::<String>()
                    .contains("oops")
            })
            .expect("the deleted line");
        let clock = buffer[(0, row)].style();
        assert!(
            clock.add_modifier.contains(Modifier::CROSSED_OUT),
            "not struck through"
        );
        assert!(
            !clock.add_modifier.contains(Modifier::DIM),
            "faded with the terminal's dim, which hides grey on black"
        );
        let rgb = as_rgb(clock.fg.expect("a colour")).expect("an rgb colour");
        assert!(luminance(rgb) >= 0.17, "{rgb:?} is invisible");
    }

    #[test]
    fn only_the_selected_message_is_drawn_reversed() {
        // A notice carries no message id, and with nothing selected both
        // sides were "nothing" — which drew every notice as if it were
        // the one being answered.
        let mut app = app_with_channel();
        app.buffers[1].push(Line {
            time: Timestamp(1_790_000_000_000),
            content: LineContent::Notice {
                level: NoticeLevel::Info,
                text: "the stream has started".into(),
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        });
        app.buffers[1].push(message_line("alice", "hello", LineFlags::default()));

        let reversed = |app: &App, needle: &str| {
            let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, app);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height).any(|y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                row.contains(needle)
                    && (0..buffer.area.width).any(|x| {
                        buffer[(x, y)]
                            .style()
                            .add_modifier
                            .contains(Modifier::REVERSED)
                    })
            })
        };
        assert!(!reversed(&app, "stream has started"), "notice was reversed");

        // The one that is selected still is.
        app.buffers[1].selected = Some("m1".into());
        assert!(reversed(&app, "hello"), "the selected message was not");
        assert!(!reversed(&app, "stream has started"), "notice was reversed");
    }

    #[test]
    fn the_user_list_shows_who_talked() {
        let mut app = app_with_channel();
        // Same person twice, and a broadcaster who spoke first.
        app.buffers[1].push(message_line("zoe", "hi", LineFlags::default()));
        app.buffers[1].push(message_line("zoe", "again", LineFlags::default()));
        app.buffers[1].push(message_line("alice", "hello", LineFlags::default()));
        app.show_users = true;

        let view = screen(&app, 80, 8);
        // alice and zoe, each once, however often they talked.
        assert!(view.iter().any(|row| row.contains("2 active")), "{view:?}");
        let panel: String = view.join("\n");
        assert!(panel.contains("@alice"), "{view:?}");
        assert!(panel.contains("@zoe"), "{view:?}");

        // Off again, and the chat gets the width back.
        app.show_users = false;
        let view = screen(&app, 80, 8);
        assert!(!view.iter().any(|row| row.contains("active")), "{view:?}");
    }

    #[test]
    fn the_user_list_gives_way_in_a_narrow_window() {
        let mut app = app_with_channel();
        app.show_users = true;
        assert_eq!(users_width(&app, 80), 16);
        // Too narrow: the chat keeps everything.
        assert_eq!(users_width(&app, 40), 0);
        // The status buffer has no users.
        app.active = 0;
        assert_eq!(users_width(&app, 80), 0);
    }

    /// The screen as cells, one string per column, so a wide character
    /// occupies the column it is in and the padding column after it.
    fn cells(app: &App, width: u16, height: u16) -> Vec<Vec<String>> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn wide_characters_do_not_push_the_user_list_out_of_place() {
        // Terminals differ over the width of emoji, and text measured one
        // column too narrow would push the column beside it out of place.
        let mut app = app_with_channel();
        app.show_users = true;
        for text in [
            "dlouhý řádek 🧁🎮🚀 s emoji",
            "日本語テキストのながいぎょう、おりかえしのテスト",
            "mixed ěščřž 日本語 🧁 and plain words to force a wrap somewhere",
        ] {
            app.buffers[1].push(message_line("someone", text, LineFlags::default()));
        }
        for width in [60u16, 80, 100] {
            let rows = cells(&app, width, 12);
            // The rule the user list draws must be in one column, on every
            // row of the chat area (all but the header and the prompt).
            let separator = rows[1]
                .iter()
                .position(|cell| cell == "│")
                .unwrap_or_else(|| panic!("no separator at width {width}: {:?}", rows[1]));
            for row in &rows[1..rows.len() - 1] {
                assert_eq!(
                    row[separator], "│",
                    "the separator moved at width {width}: {row:?}"
                );
            }
        }
    }

    #[test]
    fn the_interface_speaks_the_chosen_language() {
        let mut app = app_with_channel();
        app.show_users = true;
        let english = screen(&app, 80, 8).join("\n");
        assert!(english.contains("status"), "{english}");
        assert!(english.contains("active"), "{english}");
        assert!(english.contains("not logged in"), "{english}");

        app.language = liveside_client::lang::Language::Czech;
        let czech = screen(&app, 80, 8).join("\n");
        assert!(czech.contains("stav"), "{czech}");
        assert!(czech.contains("aktivních"), "{czech}");
        assert!(czech.contains("nepřihlášen"), "{czech}");
        // Channel names are names, not words to translate.
        assert!(czech.contains("#somechannel"), "{czech}");
    }

    #[test]
    fn the_active_tab_stays_visible_with_many_channels() {
        let mut app = app_with_channel();
        for i in 2..12 {
            app.apply(ServerMessage::Event {
                seq: liveside_proto::Seq(i),
                event: Event::ChannelJoined {
                    channel: Channel {
                        id: i.to_string().into(),
                        login: format!("channel{i:02}").into(),
                        display_name: format!("channel{i:02}"),
                    },
                    modes: RoomModes::default(),
                },
            });
        }
        // Reading the last channel of twelve, in a narrow window.
        app.active = app.buffers.len() - 1;
        let view = screen(&app, 60, 6);
        assert!(
            view[0].contains("#channel11"),
            "the active tab is not shown: {:?}",
            view[0]
        );
        assert!(
            view[0].contains('‹'),
            "no sign of the hidden tabs: {:?}",
            view[0]
        );

        // Back at the first channel, the tabs after it are reachable.
        app.active = 1;
        let view = screen(&app, 60, 6);
        assert!(view[0].contains("#somechannel"), "{:?}", view[0]);
        assert!(view[0].contains('›'), "{:?}", view[0]);

        // Everything fits when there is room, so no markers at all.
        let view = screen(&app, 200, 6);
        assert!(
            !view[0].contains('‹') && !view[0].contains('›'),
            "{:?}",
            view[0]
        );
    }

    #[test]
    fn the_settings_box_holds_its_contents() {
        use liveside_client::settings::Settings;
        let mut app = app_with_channel();
        app.settings = Settings::new(
            liveside_client::Config::default(),
            "/home/someone/.config/liveside/config.toml".into(),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        app.settings.open();
        // A narrow window is where text spills out of the box.
        for width in [40u16, 60, 80] {
            let view = screen(&app, width, 18);
            for row in &view {
                assert!(
                    row.chars().count() <= usize::from(width),
                    "row wider than the window at {width}: {row:?}"
                );
            }
            // The border is drawn on both sides of every row of the box.
            let bordered: Vec<&String> = view.iter().filter(|row| row.contains('│')).collect();
            assert!(
                bordered.iter().all(|row| row.matches('│').count() == 2),
                "a row is missing a border at {width}: {bordered:?}"
            );
        }
    }

    #[test]
    fn wide_letters_keep_the_settings_box_in_line() {
        use liveside_client::lang::Language;
        use liveside_client::settings::Settings;
        let mut app = app_with_channel();
        app.language = Language::Korean;
        // Wide letters in the chat behind the box, where its border goes.
        for _ in 0..12 {
            app.buffers[1].push(message_line(
                "someone",
                &"가".repeat(60),
                LineFlags::default(),
            ));
        }
        app.settings = Settings::new(
            liveside_client::Config::default(),
            "/home/someone/.config/liveside/config.toml".into(),
            Language::Korean,
            liveside_client::settings::Frontend::Terminal,
        );
        app.settings.open();
        // The box's left edge falls in an even column or an odd one, and
        // the letters behind it start in even ones.
        for width in [99u16, 100, 101] {
            wide_letters_in_line(&app, width);
        }
    }

    fn wide_letters_in_line(app: &App, width: u16) {
        let rows = cells(app, width, 24);
        let top = rows
            .iter()
            .position(|row| row.iter().any(|cell| cell == "┌"))
            .expect("the box is drawn");
        let left = rows[top].iter().position(|cell| cell == "┌").unwrap();
        let bottom = rows
            .iter()
            .position(|row| row.get(left).is_some_and(|cell| cell == "└"))
            .expect("the box is closed");
        for row in &rows[top + 1..bottom] {
            assert_eq!(row[left], "│", "no left border at {width}: {row:?}");
            // A wide letter just left of it would reach over the border,
            // and the terminal would never be told to draw the border.
            assert!(
                row[left - 1].width() <= 1,
                "a wide letter covers the border at {width}: {row:?}"
            );
        }
        // Each value starts in the same column, however wide its label.
        let value_at = |label: &str, value: &str| {
            let row = rows
                .iter()
                .find(|row| row.get(left + 2).is_some_and(|cell| cell == label))
                .unwrap_or_else(|| panic!("no row for {label}"));
            row.iter()
                .skip(left + 3)
                .position(|cell| cell == value)
                .unwrap_or_else(|| panic!("no {value} beside {label}: {row:?}"))
        };
        let channels = value_at("채", "(");
        assert_eq!(value_at("사", "a"), channels, "user list at {width}");
        assert_eq!(value_at("외", "켬"), channels, "emotes at {width}");
    }

    #[test]
    fn a_backend_that_is_not_twitch_is_marked() {
        let mut app = app_with_channel();
        app.backend = "demo".into();
        let view = screen(&app, 70, 6);
        assert!(view[0].contains("DEMO"), "{:?}", view[0]);

        // The real thing needs no badge.
        app.backend = "twitch".into();
        let view = screen(&app, 70, 6);
        assert!(!view[0].contains("TWITCH"), "{:?}", view[0]);
    }

    #[test]
    fn room_modes_are_labelled() {
        assert!(mode_labels(&RoomModes::default()).is_empty());
        let modes = RoomModes {
            emote_only: true,
            followers_only: Some(30),
            subscribers_only: true,
            slow: Some(10),
            unique_chat: true,
        };
        assert_eq!(
            mode_labels(&modes),
            ["sub", "emote", "follow 30m", "slow 10s", "unique"]
        );
        // Longer follow times read better in hours and days.
        let hours = RoomModes {
            followers_only: Some(120),
            ..Default::default()
        };
        assert_eq!(mode_labels(&hours), ["follow 2h"]);
        let days = RoomModes {
            followers_only: Some(2880),
            ..Default::default()
        };
        assert_eq!(mode_labels(&days), ["follow 2d"]);
        // Followers-only with no minimum time.
        let any = RoomModes {
            followers_only: Some(0),
            ..Default::default()
        };
        assert_eq!(mode_labels(&any), ["follow"]);
    }

    #[test]
    fn header_shows_active_channel_modes() {
        let mut app = app_with_channel();
        app.buffers[1].modes = RoomModes {
            subscribers_only: true,
            slow: Some(30),
            ..Default::default()
        };
        let view = screen(&app, 70, 6);
        assert!(view[0].contains("sub slow 30s"), "{:?}", view[0]);

        // The status buffer has no channel, so no modes.
        app.active = 0;
        let view = screen(&app, 70, 6);
        assert!(!view[0].contains("slow"), "{:?}", view[0]);
    }

    #[test]
    fn shows_tabs_message_and_prompt() {
        let mut app = app_with_channel();
        app.input.text = "typing".into();
        app.input.cursor = app.input.text.len();
        let screen = screen(&app, 60, 6);

        assert!(screen[0].contains("1:status"), "{:?}", screen[0]);
        assert!(screen[0].contains("2:#somechannel"), "{:?}", screen[0]);
        assert!(screen[0].contains("not logged in"), "{:?}", screen[0]);
        let chat = screen[1..5].join("\n");
        assert!(chat.contains("@ alice: hello there Kappa"), "{chat}");
        assert_eq!(screen[5], "[#somechannel] typing");
    }

    #[test]
    fn a_wrapped_link_is_found_on_both_rows_as_one_link() {
        let mut app = app_with_channel();
        let url = "https://example.com/a/rather/long/path";
        let mut line = message_line("bob", "see", LineFlags::default());
        if let LineContent::Message(message) = &mut line.content {
            message.fragments = vec![
                Fragment::Text {
                    text: "see ".into(),
                },
                Fragment::Link {
                    text: url.into(),
                    url: url.into(),
                },
            ];
        }
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(2),
            event: Event::Line {
                channel: "1001".into(),
                line,
            },
        });
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let mut links = Vec::new();
        terminal
            .draw(|frame| {
                links = draw(frame, &app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        assert!(links.len() >= 2, "{links:?}");
        assert!(
            links
                .iter()
                .all(|link| link.url == url && link.id == links[0].id)
        );
        assert!(
            links.windows(2).all(|pair| pair[0].y < pair[1].y),
            "{links:?}"
        );
        // The cells under the pieces read the address, nothing more.
        let shown: String = links
            .iter()
            .flat_map(|link| (link.x..link.x + link.width).map(move |x| (x, link.y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_owned())
            .collect();
        assert_eq!(shown, url);
    }

    #[test]
    fn a_deleted_message_has_no_link_to_click() {
        let mut line = message_line("bob", "see", LineFlags::default());
        if let LineContent::Message(message) = &mut line.content {
            message.fragments = vec![Fragment::Link {
                text: "https://a.cz/".into(),
                url: "https://a.cz/".into(),
            }];
        }
        line.flags.deleted = true;
        let strings = liveside_client::lang::Language::English.strings();
        let (_, links) = render_line(&line, strings, &Reports::new());
        assert!(links.iter().all(Option::is_none));
    }

    #[test]
    fn newest_lines_win_when_the_screen_is_full() {
        let mut app = app_with_channel();
        for i in 0..50 {
            app.apply(ServerMessage::Event {
                seq: liveside_proto::Seq(i + 2),
                event: Event::Line {
                    channel: "1001".into(),
                    line: message_line("bob", &format!("message {i}"), LineFlags::default()),
                },
            });
        }
        let view = screen(&app, 60, 6);
        let chat = view[1..5].join("\n");
        assert!(chat.contains("message 49"), "{chat}");
        assert!(!chat.contains("message 40"), "{chat}");

        // Scrolled back, older lines show and a marker appears.
        app.buffers[1].scroll = 20;
        let view = screen(&app, 60, 6);
        let chat = view[1..5].join("\n");
        assert!(chat.contains("message 26"), "{chat}");
        assert!(!chat.contains("message 49"), "{chat}");
        assert!(view[4].contains("scrolled back"), "{:?}", view[4]);
    }

    #[test]
    fn deleted_and_notice_lines() {
        let mut app = app_with_channel();
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(2),
            event: Event::Line {
                channel: "1001".into(),
                line: Line {
                    time: Timestamp(1_790_000_000_000),
                    content: LineContent::Notice {
                        level: NoticeLevel::Error,
                        text: "something broke".into(),
                        code: String::new(),
                        args: Vec::new(),
                    },
                    flags: LineFlags::default(),
                },
            },
        });
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(3),
            event: Event::UserCleared {
                channel: "1001".into(),
                user: "1".into(),
                seconds: Some(600),
            },
        });
        let view = screen(&app, 60, 6);
        let chat = view[1..5].join("\n");
        assert!(chat.contains("-- something broke"), "{chat}");
        assert!(chat.contains("alice"), "{chat}");
    }
}
