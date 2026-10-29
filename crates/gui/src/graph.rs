//! How many have watched a stream, as a graph: the engine's minute by
//! minute samples, with the changes of title or category marked, the way
//! Chatty's stream graph shows them. Pointing at the graph says how many
//! watched then.
//!
//! The window stays open and takes samples as they come.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill, grouped, stream_mark, viewer_summary};
use liveside_proto::UserId;
use liveside_proto::model::ViewerHistory;

use crate::theme::Palette;

/// Room for the axis labels.
const LEFT: f64 = 52.0;
const BOTTOM: f64 = 22.0;
const TOP: f64 = 16.0;
const RIGHT: f64 = 12.0;

pub struct GraphWindow {
    window: gtk::Window,
    channel: UserId,
    history: Rc<RefCell<ViewerHistory>>,
    area: gtk::DrawingArea,
    summary: gtk::Label,
    marks: gtk::Label,
    strings: &'static Strings,
    open: Rc<Cell<bool>>,
}

impl GraphWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        channel: UserId,
        login: &str,
        history: ViewerHistory,
        palette: &'static Palette,
        strings: &'static Strings,
    ) -> Rc<Self> {
        let history = Rc::new(RefCell::new(history));
        let hover: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
        let area = gtk::DrawingArea::builder()
            .content_height(260)
            .vexpand(true)
            .hexpand(true)
            .build();
        let point = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label"])
            .label(" ")
            .build();
        area.set_draw_func({
            let (history, hover) = (history.clone(), hover.clone());
            move |_, cr, width, height| {
                draw(cr, width, height, &history.borrow(), hover.get(), palette);
            }
        });
        // Pointing says how many watched at that minute.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let (history, hover, area, point) =
                (history.clone(), hover.clone(), area.clone(), point.clone());
            move |_, x, _| {
                hover.set(Some(x));
                let width = f64::from(area.width());
                point.set_text(
                    &nearest(&history.borrow(), x, width)
                        .map(|s| {
                            fill(
                                strings.graph_point,
                                &[
                                    ("time", &crate::chat::clock(s.time)),
                                    ("viewers", &grouped(s.viewers)),
                                ],
                            )
                        })
                        .unwrap_or_else(|| " ".to_owned()),
                );
                area.queue_draw();
            }
        });
        motion.connect_leave({
            let (hover, area, point) = (hover.clone(), area.clone(), point.clone());
            move |_| {
                hover.set(None);
                point.set_text(" ");
                area.queue_draw();
            }
        });
        area.add_controller(motion);

        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        let marks = gtk::Label::builder().xalign(0.0).wrap(true).build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.set_margin_top(10);
        column.set_margin_bottom(10);
        column.append(&summary);
        column.append(&area);
        column.append(&point);
        column.append(&marks);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(fill(strings.graph_title, &[("channel", login)]))
            .default_width(640)
            .default_height(440)
            .child(&column)
            .build();
        crate::dialog::close_on_escape(&window);
        let open = Rc::new(Cell::new(true));
        window.connect_close_request({
            let open = open.clone();
            move |_| {
                open.set(false);
                gtk::glib::Propagation::Proceed
            }
        });
        let this = Rc::new(Self {
            window,
            channel,
            history,
            area,
            summary,
            marks,
            strings,
            open,
        });
        this.show_words();
        this.window.present();
        this
    }

    pub fn channel(&self) -> &UserId {
        &self.channel
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// The history as it now stands.
    pub fn update(&self, history: &ViewerHistory) {
        *self.history.borrow_mut() = history.clone();
        self.show_words();
        self.area.queue_draw();
    }

    fn show_words(&self) {
        let history = self.history.borrow();
        self.summary
            .set_text(&viewer_summary(self.strings, &history, crate::chat::clock));
        let marks: Vec<String> = history
            .marks
            .iter()
            .enumerate()
            .map(|(i, mark)| {
                format!(
                    "{}. {} {}",
                    i + 1,
                    crate::chat::clock(mark.time),
                    stream_mark(mark)
                )
            })
            .collect();
        self.marks.set_visible(!marks.is_empty());
        self.marks.set_text(&format!(
            "{}\n{}",
            self.strings.graph_marks,
            marks.join("\n")
        ));
    }
}

/// The span of time the graph covers, in milliseconds.
fn span(history: &ViewerHistory) -> Option<(i64, i64)> {
    let first = history.samples.first()?.time.0;
    let last = history.samples.last()?.time.0;
    // One sample still makes a graph a minute wide.
    Some((first, last.max(first + 60_000)))
}

/// The sample nearest to a point across the graph.
fn nearest(
    history: &ViewerHistory,
    x: f64,
    width: f64,
) -> Option<liveside_proto::model::ViewerSample> {
    let (from, to) = span(history)?;
    let plot = (width - LEFT - RIGHT).max(1.0);
    let at = from as f64 + ((x - LEFT) / plot).clamp(0.0, 1.0) * (to - from) as f64;
    history
        .samples
        .iter()
        .copied()
        .min_by_key(|s| (s.time.0 as f64 - at).abs() as i64)
}

/// A round step for the axis: 1, 2 or 5 times a power of ten.
fn step_for(max: f64) -> f64 {
    let rough = (max / 4.0).max(1.0);
    let power = 10f64.powf(rough.log10().floor());
    let scaled = rough / power;
    let nice = if scaled <= 1.0 {
        1.0
    } else if scaled <= 2.0 {
        2.0
    } else if scaled <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * power
}

/// `#rrggbb` as cairo's colours.
fn rgb(hex: &str) -> (f64, f64, f64) {
    let hex = hex.trim_start_matches('#');
    let part = |i: usize| {
        u8::from_str_radix(hex.get(i..i + 2).unwrap_or("80"), 16).unwrap_or(128) as f64 / 255.0
    };
    (part(0), part(2), part(4))
}

fn draw(
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    history: &ViewerHistory,
    hover: Option<f64>,
    palette: &Palette,
) {
    let (width, height) = (f64::from(width), f64::from(height));
    let text = if palette.dark {
        (0.85, 0.85, 0.85)
    } else {
        (0.25, 0.25, 0.25)
    };
    let line = (0.21, 0.52, 0.89);
    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    cr.set_font_size(11.0);
    let Some((from, to)) = span(history) else {
        return;
    };
    let plot_w = (width - LEFT - RIGHT).max(1.0);
    let plot_h = (height - TOP - BOTTOM).max(1.0);
    let peak = history.peak().map(|s| s.viewers).unwrap_or(0).max(1) as f64;
    let step = step_for(peak * 1.1);
    let top = (peak * 1.1 / step).ceil() * step;
    let x_of = |t: i64| LEFT + (t - from) as f64 / (to - from) as f64 * plot_w;
    let y_of = |v: f64| TOP + plot_h - v / top * plot_h;

    // The grid and how many each line stands for.
    let mut value = 0.0;
    while value <= top + 0.5 {
        let y = y_of(value);
        cr.set_source_rgba(text.0, text.1, text.2, 0.15);
        cr.set_line_width(1.0);
        cr.move_to(LEFT, y.round() + 0.5);
        cr.line_to(LEFT + plot_w, y.round() + 0.5);
        let _ = cr.stroke();
        cr.set_source_rgb(text.0, text.1, text.2);
        let label = grouped(value as u32).replace('\u{a0}', " ");
        if let Ok(extents) = cr.text_extents(&label) {
            cr.move_to(LEFT - 6.0 - extents.width(), y + 4.0);
            let _ = cr.show_text(&label);
        }
        value += step;
    }
    // When: the start, the end, and the middle.
    for (t, align) in [(from, 0.0), ((from + to) / 2, 0.5), (to, 1.0)] {
        let label = crate::chat::clock(liveside_proto::Timestamp(t));
        if let Ok(extents) = cr.text_extents(&label) {
            cr.set_source_rgb(text.0, text.1, text.2);
            cr.move_to(x_of(t) - extents.width() * align, height - 6.0);
            let _ = cr.show_text(&label);
        }
    }

    // The changes of title or category, numbered as in the list below.
    let (mr, mg, mb) = rgb(palette.event);
    for (i, mark) in history.marks.iter().enumerate() {
        let t = mark.time.0.clamp(from, to);
        let x = x_of(t).round() + 0.5;
        cr.set_source_rgba(mr, mg, mb, 0.8);
        cr.set_dash(&[4.0, 3.0], 0.0);
        cr.move_to(x, TOP);
        cr.line_to(x, TOP + plot_h);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        cr.move_to(x + 3.0, TOP + 10.0);
        let _ = cr.show_text(&(i + 1).to_string());
    }

    // The audience: a line, and the area under it lightly filled.
    let points: Vec<(f64, f64)> = history
        .samples
        .iter()
        .map(|s| (x_of(s.time.0), y_of(f64::from(s.viewers))))
        .collect();
    if let Some(&(x0, y0)) = points.first() {
        cr.move_to(x0, TOP + plot_h);
        cr.line_to(x0, y0);
        for &(x, y) in &points[1..] {
            cr.line_to(x, y);
        }
        let (xn, _) = *points.last().unwrap_or(&(x0, y0));
        cr.line_to(xn, TOP + plot_h);
        cr.close_path();
        cr.set_source_rgba(line.0, line.1, line.2, 0.15);
        let _ = cr.fill();
        cr.move_to(x0, y0);
        for &(x, y) in &points[1..] {
            cr.line_to(x, y);
        }
        cr.set_source_rgb(line.0, line.1, line.2);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        if points.len() == 1 {
            cr.arc(x0, y0, 3.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }

    // Where the pointer is.
    if let Some(sample) = hover.and_then(|x| nearest(history, x, width)) {
        let (x, y) = (x_of(sample.time.0), y_of(f64::from(sample.viewers)));
        cr.set_source_rgba(text.0, text.1, text.2, 0.4);
        cr.set_line_width(1.0);
        cr.move_to(x.round() + 0.5, TOP);
        cr.line_to(x.round() + 0.5, TOP + plot_h);
        let _ = cr.stroke();
        cr.set_source_rgb(line.0, line.1, line.2);
        cr.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::Timestamp;
    use liveside_proto::model::StreamStats;

    #[test]
    fn the_axis_goes_in_round_steps() {
        assert_eq!(step_for(4.0), 1.0);
        assert_eq!(step_for(1357.0), 500.0);
        assert_eq!(step_for(90.0), 50.0);
        assert_eq!(step_for(30_000.0), 10_000.0);
    }

    #[test]
    fn pointing_finds_the_nearest_minute() {
        let mut history = ViewerHistory::default();
        let started = Timestamp(0);
        for (minute, viewers) in [(0, 10), (1, 20), (2, 30)] {
            history.record(
                Timestamp(minute * 60_000),
                &StreamStats { viewers, started },
            );
        }
        let width = LEFT + RIGHT + 200.0;
        assert_eq!(nearest(&history, LEFT, width).unwrap().viewers, 10);
        assert_eq!(nearest(&history, LEFT + 100.0, width).unwrap().viewers, 20);
        assert_eq!(nearest(&history, width, width).unwrap().viewers, 30);
        assert_eq!(rgb("#ff8000"), (1.0, 128.0 / 255.0, 0.0));
    }
}
