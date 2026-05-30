//! Drawing. One frame = a north-up radar canvas on the left, a flight list and a
//! status block on the right. Everything is derived from a single set of tracks
//! dead-reckoned to one instant, so the radar, list, and status never disagree.

use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::VerticalTrend;
use crate::tracker::{Health, Track};
use crate::{fmt_pacing, fmt_position, label};

use super::app::{App, Mode};

const RING_FRACTIONS: [f64; 3] = [1.0 / 3.0, 2.0 / 3.0, 1.0];

/// Terminal character cells are roughly twice as tall as they are wide. The
/// radar corrects for this so equal range means equal screen distance from Home
/// in every direction — otherwise the range rings render as ovals.
const CELL_ASPECT: f64 = 2.0;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let now = Instant::now();
    // One dead-reckoning pass feeds the whole frame, so the radar, list, and
    // status can't disagree about who is nearest, pacing, or even present.
    let picture = app.tracker.picture_at(now);
    let nearest_hex = picture.nearest().map(|t| t.flight.hex.clone());
    let pacing_hex = picture.pacing().map(|t| t.flight.hex.clone());
    let tracks = &picture.tracks;
    let health = &picture.health;

    let [radar_area, panel] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .areas(frame.area());
    let [list_area, status_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(8)]).areas(panel);

    draw_radar(
        frame,
        radar_area,
        app.area.radius_nm,
        tracks,
        nearest_hex.clone(),
        pacing_hex.clone(),
        app.selected_hex.clone(),
    );
    draw_list(
        frame,
        list_area,
        tracks,
        pacing_hex.as_deref(),
        app.selected_hex.as_deref(),
        &mut app.list_state,
    );
    draw_status(
        frame,
        status_area,
        app,
        tracks,
        health,
        now,
        nearest_hex.as_deref(),
        pacing_hex.as_deref(),
        app.selected_hex.as_deref(),
    );

    // The flight-detail popup layers over the dimmed radar+list. Clone the
    // selection so the scroll offset can be borrowed mutably for clamping.
    if app.mode == Mode::Detail {
        let selected = app.selected_hex.clone();
        draw_detail(frame, tracks, selected.as_deref(), &mut app.detail_scroll);
    }
}

fn draw_radar(
    frame: &mut Frame,
    area: Rect,
    radius: f64,
    tracks: &[Track],
    nearest_hex: Option<String>,
    pacing_hex: Option<String>,
    selected_hex: Option<String>,
) {
    // The canvas maps each axis's data range across the pane independently, so
    // equal bounds in a non-square pane stretch circles into ovals. We widen the
    // longer pixel axis's bounds to compensate: the shorter axis keeps `radius`
    // (its outer ring touches the edge) and the longer axis gains empty scope.
    let inner_w = area.width.saturating_sub(2).max(1) as f64;
    let inner_h = area.height.saturating_sub(2).max(1) as f64;
    let pixel_aspect = inner_w / inner_h / CELL_ASPECT; // pane width:height in pixels
    let (x_extent, y_extent) = if pixel_aspect >= 1.0 {
        (radius * pixel_aspect, radius)
    } else {
        (radius, radius / pixel_aspect)
    };

    let canvas = Canvas::default()
        .block(Block::bordered().title(" radar · north-up "))
        .x_bounds([-x_extent, x_extent])
        .y_bounds([-y_extent, y_extent])
        .marker(Marker::Braille)
        .paint(move |ctx| {
            // Range rings and faint cardinal cross-hairs.
            for frac in RING_FRACTIONS {
                ctx.draw(&Circle {
                    x: 0.0,
                    y: 0.0,
                    radius: radius * frac,
                    color: Color::DarkGray,
                });
            }
            let cross = Color::Rgb(40, 40, 40);
            ctx.draw(&CanvasLine {
                x1: -x_extent,
                y1: 0.0,
                x2: x_extent,
                y2: 0.0,
                color: cross,
            });
            ctx.draw(&CanvasLine {
                x1: 0.0,
                y1: -y_extent,
                x2: 0.0,
                y2: y_extent,
                color: cross,
            });
            ctx.layer();

            // Blips with heading vectors.
            for t in tracks {
                let (x, y) = radar_xy(t);
                let color = blip_color(t, &nearest_hex, &pacing_hex, &selected_hex);
                if let Some((track, gs)) = t.flight.velocity() {
                    let len = (radius * 0.05 * (gs / 300.0)).clamp(radius * 0.02, radius * 0.12);
                    let a = track.to_radians();
                    ctx.draw(&CanvasLine {
                        x1: x,
                        y1: y,
                        x2: x + a.sin() * len,
                        y2: y + a.cos() * len,
                        color,
                    });
                }
                ctx.draw(&Points {
                    coords: &[(x, y)],
                    color,
                });
            }
            ctx.layer();

            // Home at the center.
            ctx.print(
                0.0,
                0.0,
                Line::from(Span::styled(
                    "⌂",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )),
            );

            // Labels only on flagged flights (nearest / pacing / selected) so the
            // scope doesn't turn into a wall of text.
            for t in tracks {
                if !is_flagged(t, &nearest_hex, &pacing_hex, &selected_hex) {
                    continue;
                }
                let (x, y) = radar_xy(t);
                let color = blip_color(t, &nearest_hex, &pacing_hex, &selected_hex);
                ctx.print(
                    x + radius * 0.03,
                    y,
                    Line::from(Span::styled(label(&t.flight), Style::new().fg(color))),
                );
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    tracks: &[Track],
    pacing_hex: Option<&str>,
    selected_hex: Option<&str>,
    state: &mut ListState,
) {
    // Reserve the borders (2) and the highlight symbol ("▶ " = 2) from the width
    // available to each row, so the width-aware fit matches what actually shows.
    let row_width = area.width.saturating_sub(4);
    let items: Vec<ListItem> = tracks
        .iter()
        .map(|t| ListItem::new(list_row(t, pacing_hex, row_width)))
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(format!(" flights · {} ", tracks.len())))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    let index = selected_hex.and_then(|hex| tracks.iter().position(|t| t.flight.hex == hex));
    state.select(index);
    frame.render_stateful_widget(list, area, state);
}

fn list_row(t: &Track, pacing_hex: Option<&str>, max_width: u16) -> Line<'static> {
    let is_pacing = Some(t.flight.hex.as_str()) == pacing_hex;
    let ident_style = if is_pacing {
        Style::new()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD)
    } else if t.flight.ident.is_none() {
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new()
    };

    let alt = t
        .flight
        .altitude_ft
        .map(|a| format!("{a:.0}ft"))
        .unwrap_or_else(|| "?".into());

    // The ICAO type designator (e.g. B738), dimmed so it reads as secondary to
    // the callsign. Blank when the Source didn't supply one.
    let kind = t.flight.aircraft_type.as_deref().unwrap_or("");

    // Climb/descend/level glyph glued to the altitude.
    let (glyph, glyph_color) = trend_glyph(t.flight.vertical_trend());

    let gs_span = match t.flight.groundspeed_kt {
        Some(g) => Span::raw(format!(" {g:>3.0}kt")),
        None => Span::raw(String::new()),
    };

    let (arrow, arrow_color, cpa) = match t.cpa {
        Some(c) if c.is_approaching() => (
            "▲",
            Color::Green,
            format!("{:.0}nm/{:.0}s", c.cpa_distance_nm, c.time_to_cpa_s),
        ),
        Some(_) => ("▼", Color::Blue, String::new()),
        None => ("·", Color::DarkGray, String::new()),
    };

    // (priority, group): when the row can't fit, the highest-priority-number
    // groups are dropped whole, lowest-priority (highest number) first. A group is
    // one or more spans kept or dropped together — the altitude and its trend glyph
    // share a group so the glyph never shows orphaned, and likewise the CPA arrow
    // and its text. Kept groups render in this (display) order. Keep, in order:
    // callsign, distance/bearing, altitude+glyph, groundspeed, type, then CPA.
    let groups = vec![
        (
            0,
            vec![Span::styled(format!("{:<8}", label(&t.flight)), ident_style)],
        ),
        (
            4,
            vec![Span::styled(
                format!(" {kind:<4}"),
                Style::new().fg(Color::DarkGray),
            )],
        ),
        (
            1,
            vec![Span::raw(format!(
                " {:>5.1}nm {:03.0}°",
                t.distance_nm, t.bearing_from_home
            ))],
        ),
        (
            2,
            vec![
                Span::raw(format!(" {alt:>7}")),
                Span::styled(glyph.to_string(), Style::new().fg(glyph_color)),
            ],
        ),
        (3, vec![gs_span]),
        (
            5,
            vec![
                Span::styled(format!(" {arrow} "), Style::new().fg(arrow_color)),
                Span::raw(cpa),
            ],
        ),
    ];
    fit_groups(groups, max_width)
}

/// The list glyph and colour for a vertical trend. Unknown renders blank rather
/// than a fake symbol.
fn trend_glyph(trend: VerticalTrend) -> (&'static str, Color) {
    match trend {
        VerticalTrend::Climb => ("↑", Color::Green),
        VerticalTrend::Descend => ("↓", Color::Cyan),
        VerticalTrend::Level => ("–", Color::DarkGray),
        VerticalTrend::Unknown => ("", Color::DarkGray),
    }
}

/// Assemble row groups to fit `max` columns. Each group carries a priority and one
/// or more spans kept or dropped together; when the full row is too wide, groups
/// are dropped lowest-priority (highest number) first. Whatever survives renders in
/// the original order, so the row degrades gracefully on a narrow terminal instead
/// of clipping its rightmost (newest) data — and a multi-span group (altitude +
/// glyph, arrow + CPA) never half-survives.
fn fit_groups(groups: Vec<(u8, Vec<Span<'static>>)>, max: u16) -> Line<'static> {
    let mut keep = vec![false; groups.len()];
    let mut used = 0u16;
    let mut by_priority: Vec<usize> = (0..groups.len()).collect();
    by_priority.sort_by_key(|&i| groups[i].0);
    for i in by_priority {
        let w: u16 = groups[i].1.iter().map(|s| s.width() as u16).sum();
        if used.saturating_add(w) <= max {
            keep[i] = true;
            used = used.saturating_add(w);
        }
    }
    let kept: Vec<Span<'static>> = groups
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .flat_map(|(_, (_, spans))| spans)
        .collect();
    Line::from(kept)
}

#[allow(clippy::too_many_arguments)]
fn draw_status(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    tracks: &[Track],
    health: &Health,
    now: Instant,
    nearest_hex: Option<&str>,
    pacing_hex: Option<&str>,
    selected_hex: Option<&str>,
) {
    let (state_label, state_color, health_error) = match health {
        Health::Live => ("LIVE", Color::Green, None),
        Health::Stale { last_error } => ("STALE", Color::Yellow, last_error.as_deref()),
        Health::NoData { last_error } => ("NO DATA", Color::Red, last_error.as_deref()),
    };
    let age = app
        .tracker
        .snapshot_age(now)
        .map(|d| format!("{:.0}s ago", d.as_secs_f64()))
        .unwrap_or_else(|| "—".into());

    let find = |hex: Option<&str>| hex.and_then(|h| tracks.iter().find(|t| t.flight.hex == h));
    let nearest = find(nearest_hex)
        .map(fmt_position)
        .unwrap_or_else(|| "(none)".into());
    let pacing = find(pacing_hex)
        .map(fmt_pacing)
        .unwrap_or_else(|| "quiet — backing off".into());
    let countdown = app
        .next_poll_at
        .map(|t| format!("{:.1}s", t.saturating_duration_since(now).as_secs_f64()))
        .unwrap_or_else(|| "—".into());

    // The selected flight's live (dead-reckoned) coordinates and how old the
    // underlying position report now is — useful for cross-referencing.
    let selected = find(selected_hex).map(|t| {
        // Prefer the full model description; fall back to the type code, then to
        // a placeholder when the Source gave us neither.
        let model = t
            .flight
            .model
            .as_deref()
            .or(t.flight.aircraft_type.as_deref())
            .unwrap_or("unknown type");
        format!(
            "sel {} · {model}  {:.3}, {:.3}  ({:.0}s old)",
            label(&t.flight),
            t.estimated.lat,
            t.estimated.lon,
            t.age.as_secs_f64()
        )
    });

    let mut lines = vec![
        Line::from(vec![
            Span::raw("source "),
            Span::styled(
                app.source_name.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("[{state_label}]"),
                Style::new().fg(state_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} flights · {age}", tracks.len())),
        ]),
        Line::from(format!("nearest  {nearest}")),
        Line::from(format!("pacing   {pacing}")),
        Line::from(format!(
            "next poll in {countdown}   relevance {:.0}nm",
            app.relevance_nm
        )),
    ];
    match selected {
        Some(s) => lines.push(Line::from(Span::styled(s, Style::new().fg(Color::White)))),
        None => lines.push(Line::from(Span::styled(
            "↑/↓ select · Enter detail · Esc clear · q quit",
            Style::new().fg(Color::DarkGray),
        ))),
    }
    if let Some(err) = health_error {
        lines.push(Line::from(Span::styled(
            format!("! {err}"),
            Style::new().fg(Color::Red),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" status ")),
        area,
    );
}

/// The flight-detail popup: dim the frame, clear a centered box, and fill it with
/// the inspected flight's data — or a "left the area" notice if it has dropped out
/// of the Search area while the popup was open (never fabricated values). The key
/// hints sit on a fixed bottom line so scrolling the body never loses them.
fn draw_detail(frame: &mut Frame, tracks: &[Track], selected_hex: Option<&str>, scroll: &mut u16) {
    let full = frame.area();
    frame
        .buffer_mut()
        .set_style(full, Style::new().add_modifier(Modifier::DIM));

    let area = centered_rect(full, 64, 80, 78, 32);
    frame.render_widget(Clear, area);

    let block = Block::bordered().title(" flight detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Reserve the bottom line of the popup for the always-visible key hints; the
    // flight data scrolls in the space above it.
    let [body_area, footer_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let track = selected_hex.and_then(|h| tracks.iter().find(|t| t.flight.hex == h));
    let (body, footer) = match track {
        Some(t) => (detail_lines(t), "Esc close · ↑/↓ flight · PgUp/PgDn scroll"),
        None => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  — flight left the area —",
                    Style::new().fg(Color::DarkGray),
                )),
            ],
            "Esc close · ↑/↓ next flight",
        ),
    };

    // Clamp so the body can't be scrolled entirely past the viewport, leaving a
    // void below it. Bounds against the logical line count (the detail fields are
    // short and never wrap at the popup width, so this matches the rendered height).
    let max_scroll = (body.len() as u16).saturating_sub(body_area.height);
    *scroll = (*scroll).min(max_scroll);

    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll((*scroll, 0)),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::new().fg(Color::DarkGray),
        ))),
        footer_area,
    );
}

/// Every displayable field for one flight: a header, the promoted typed fields
/// grouped under "Position & motion" / "Transponder", then the Source-contributed
/// [`crate::domain::DetailGroup`]s rendered generically (no per-Source code).
fn detail_lines(t: &Track) -> Vec<Line<'static>> {
    let f = &t.flight;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(Span::styled(
        label(f),
        Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    if let Some(r) = &f.registration {
        lines.push(field_line("Registration", r));
    }
    if let Some(o) = &f.operator {
        lines.push(field_line("Operator", o));
    }
    let kind = match (&f.aircraft_type, &f.model) {
        (Some(ty), Some(m)) => Some(format!("{ty} · {m}")),
        (Some(ty), None) => Some(ty.clone()),
        (None, Some(m)) => Some(m.clone()),
        (None, None) => None,
    };
    if let Some(k) = kind {
        lines.push(field_line("Type", &k));
    }
    if let Some(c) = &f.emitter_category {
        lines.push(field_line("Category", c));
    }

    section_title(&mut lines, "Position & motion");
    lines.push(field_line("Distance", &format!("{:.1} nm", t.distance_nm)));
    lines.push(field_line("Bearing", &format!("{:03.0}°", t.bearing_from_home)));
    if let Some(a) = f.altitude_ft {
        lines.push(field_line("Baro alt", &format!("{a:.0} ft")));
    }
    if let Some(a) = f.geometric_altitude_ft {
        lines.push(field_line("Geo alt", &format!("{a:.0} ft")));
    }
    if let Some(r) = f.vertical_rate_fpm {
        let (glyph, _) = trend_glyph(f.vertical_trend());
        lines.push(field_line("Vertical rate", &format!("{r:+.0} fpm {glyph}")));
    }
    if let Some(g) = f.groundspeed_kt {
        lines.push(field_line("Groundspeed", &format!("{g:.0} kt")));
    }
    if let Some(tr) = f.track_deg {
        lines.push(field_line("Track", &format!("{tr:03.0}°")));
    }
    lines.push(field_line(
        "Position",
        &format!("{:.4}, {:.4}", t.estimated.lat, t.estimated.lon),
    ));
    lines.push(field_line(
        "Position age",
        &format!("{:.0}s", t.age.as_secs_f64()),
    ));

    if f.squawk.is_some() || f.emergency.is_some() {
        section_title(&mut lines, "Transponder");
        if let Some(s) = &f.squawk {
            lines.push(field_line("Squawk", s));
        }
        if let Some(e) = &f.emergency {
            lines.push(field_line("Emergency", e));
        }
    }

    // The opaque, Source-formatted detail groups — rendered verbatim.
    for group in &f.details {
        section_title(&mut lines, &group.title);
        for (label, value) in &group.fields {
            lines.push(field_line(label, value));
        }
    }

    lines
}

/// A blank spacer then a styled section heading.
fn section_title(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    )));
}

/// One `label   value` row inside the popup.
fn field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<16}"),
            Style::new().fg(Color::DarkGray),
        ),
        Span::raw(value.to_string()),
    ])
}

/// A box of `pct_x`×`pct_y` percent of `area`, capped at `max_x`×`max_y` cells,
/// centered within `area`.
fn centered_rect(area: Rect, pct_x: u16, pct_y: u16, max_x: u16, max_y: u16) -> Rect {
    let w = ((area.width as u32 * pct_x as u32 / 100) as u16)
        .min(max_x)
        .min(area.width);
    let h = ((area.height as u32 * pct_y as u32 / 100) as u16)
        .min(max_y)
        .min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// A flight's radar position as (east, north) nm offsets from Home.
fn radar_xy(t: &Track) -> (f64, f64) {
    let b = t.bearing_from_home.to_radians();
    (t.distance_nm * b.sin(), t.distance_nm * b.cos())
}

fn is_flagged(
    t: &Track,
    nearest: &Option<String>,
    pacing: &Option<String>,
    selected: &Option<String>,
) -> bool {
    let hex = Some(t.flight.hex.clone());
    hex == *nearest || hex == *pacing || hex == *selected
}

/// Blip color by priority: selected, then pacing, then nearest, then approach
/// state. Anonymous (ident-blocked) flights are dimmed unless flagged.
fn blip_color(
    t: &Track,
    nearest: &Option<String>,
    pacing: &Option<String>,
    selected: &Option<String>,
) -> Color {
    let hex = t.flight.hex.as_str();
    if selected.as_deref() == Some(hex) {
        return Color::White;
    }
    if pacing.as_deref() == Some(hex) {
        return Color::LightRed;
    }
    if nearest.as_deref() == Some(hex) {
        return Color::Cyan;
    }
    if t.flight.ident.is_none() {
        return Color::DarkGray;
    }
    match t.cpa {
        Some(c) if c.is_approaching() => Color::Green,
        _ => Color::Blue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Flight, LatLon, SearchArea, Snapshot};
    use crate::poller::PollUpdate;
    use crate::tracker::TrackerConfig;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::{Duration, Instant};

    fn moving(hex: &str, ident: &str, pos: LatLon, track: f64, gs: f64) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some(ident.into()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: None,
            operator: None,
            position: pos,
            altitude_ft: Some(30_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(gs),
            track_deg: Some(track),
            vertical_rate_fpm: Some(1200.0),
            squawk: None,
            emergency: None,
            emitter_category: None,
            reported_age: Duration::ZERO,
            details: Vec::new(),
        }
    }

    /// Render one real frame to an in-memory backend and assert the radar, list,
    /// and status all painted — the path that can't be driven interactively in CI.
    #[test]
    fn draws_radar_list_and_status_with_nearest_and_pacing() {
        let area = SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        };
        let cfg = TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        };
        let mut app = App::new(
            area,
            cfg,
            "testsrc".into(),
            Duration::from_millis(250),
            30.0,
        );

        // Overhead but receding → the Nearest flight, not the Pacing flight.
        let overhead = moving("aaa111", "OVERHEAD", LatLon::new(0.03, 0.0), 0.0, 200.0);
        // Inbound from the south, CPA imminent and within relevance → Pacing.
        let inbound = moving("bbb222", "INBOUND", LatLon::new(-0.4, 0.0), 0.0, 400.0);
        app.on_update(PollUpdate::Snapshot {
            snapshot: Snapshot::new(vec![overhead, inbound], Instant::now()),
            next_interval: Duration::from_secs(1),
        });

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();

        for needle in [
            "radar", "flights", "status", "testsrc", "LIVE", "nearest", "pacing", "OVERHEAD",
            "INBOUND", "select", "B738", // the aircraft type code shows in the list
        ] {
            assert!(text.contains(needle), "rendered frame missing {needle:?}");
        }
    }

    fn test_app() -> App {
        let area = SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        };
        let cfg = TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        };
        App::new(area, cfg, "testsrc".into(), Duration::from_millis(250), 30.0)
    }

    fn render_to_text(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn ingest_one(app: &mut App, f: Flight) {
        app.on_update(PollUpdate::Snapshot {
            snapshot: Snapshot::new(vec![f], Instant::now()),
            next_interval: Duration::from_secs(1),
        });
    }

    #[test]
    fn popup_paints_detail_in_detail_mode() {
        let mut app = test_app();
        let mut f = moving("sel123", "SELFLT", LatLon::new(0.05, 0.0), 90.0, 452.0);
        f.registration = Some("N12345".into());
        f.details = vec![crate::domain::DetailGroup {
            title: "Signal".into(),
            fields: vec![("RSSI".into(), "-7.4 dBFS".into())],
        }];
        ingest_one(&mut app, f);
        app.selected_hex = Some("sel123".into());
        app.mode = Mode::Detail;

        let text = render_to_text(&mut app, 120, 40);
        for needle in [
            "flight detail",
            "SELFLT",
            "N12345",
            "Position & motion",
            "Signal",
            "RSSI",
            "452 kt",
        ] {
            assert!(text.contains(needle), "popup missing {needle:?}");
        }
    }

    #[test]
    fn popup_shows_left_the_area_when_flight_absent() {
        let mut app = test_app();
        ingest_one(
            &mut app,
            moving("present", "PRESENT", LatLon::new(0.05, 0.0), 90.0, 300.0),
        );
        app.selected_hex = Some("ghost".into()); // never in the Snapshot
        app.mode = Mode::Detail;

        let text = render_to_text(&mut app, 120, 40);
        assert!(
            text.contains("flight left the area"),
            "expected the left-the-area notice"
        );
    }

    #[test]
    fn list_row_survives_a_narrow_terminal() {
        let mut app = test_app();
        ingest_one(
            &mut app,
            moving("aaa111", "TIGHTONE", LatLon::new(0.05, 0.0), 90.0, 452.0),
        );
        // Must not panic; the callsign (highest priority) survives truncation.
        let text = render_to_text(&mut app, 40, 20);
        assert!(text.contains("TIGHTONE"), "callsign should survive");
    }

    #[test]
    fn list_row_drops_altitude_and_its_trend_glyph_together() {
        // `moving` builds a climbing flight (1200 fpm → "↑") at 30000 ft. Altitude
        // and its glyph share one fit group, so at a width that can't hold the
        // altitude the glyph must not survive alone — the bug this grouping fixes.
        let track = Track {
            flight: moving("aaa111", "TIGHT", LatLon::new(0.05, 0.0), 90.0, 452.0),
            estimated: LatLon::new(0.05, 0.0),
            distance_nm: 3.0,
            bearing_from_home: 90.0,
            cpa: None,
            age: Duration::ZERO,
        };
        // Fits the callsign (8) and distance/bearing (13) but not the altitude
        // group (9); a lone 1-col glyph would otherwise sneak in.
        let line = list_row(&track, None, 22);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("TIGHT"), "callsign survives");
        assert!(!text.contains("30000"), "altitude is dropped at this width");
        assert!(
            !text.contains('↑'),
            "trend glyph must not survive without its altitude"
        );
    }

    #[test]
    fn fit_groups_drops_whole_groups_keeping_priority_order() {
        // Three groups; the priority-2 group has two spans and must be kept or
        // dropped as a unit. At width 8 the two lowest-number priorities fit and
        // render in original (display) order; the priority-2 group drops whole.
        let groups = vec![
            (0u8, vec![Span::raw("aaaa")]),
            (2, vec![Span::raw("bb"), Span::raw("BB")]),
            (1, vec![Span::raw("cccc")]),
        ];
        let line = fit_groups(groups, 8);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "aaaacccc");
    }
}
