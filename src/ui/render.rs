//! Drawing. One frame = a north-up radar canvas on the left, a flight list and a
//! status block on the right. Everything is derived from a single set of tracks
//! dead-reckoned to one instant, so the radar, list, and status never disagree.

use std::time::Instant;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tracker::{Health, Track};
use crate::{fmt_pacing, fmt_position, label};

use super::app::App;

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
    let items: Vec<ListItem> = tracks
        .iter()
        .map(|t| ListItem::new(list_row(t, pacing_hex)))
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(format!(" flights · {} ", tracks.len())))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▶ ");

    let index = selected_hex.and_then(|hex| tracks.iter().position(|t| t.flight.hex == hex));
    state.select(index);
    frame.render_stateful_widget(list, area, state);
}

fn list_row<'a>(t: &Track, pacing_hex: Option<&str>) -> Line<'a> {
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

    let (arrow, arrow_color, cpa) = match t.cpa {
        Some(c) if c.is_approaching() => (
            "▲",
            Color::Green,
            format!("{:.0}nm/{:.0}s", c.cpa_distance_nm, c.time_to_cpa_s),
        ),
        Some(_) => ("▼", Color::Blue, String::new()),
        None => ("·", Color::DarkGray, String::new()),
    };

    Line::from(vec![
        Span::styled(format!("{:<8}", label(&t.flight)), ident_style),
        Span::styled(format!(" {kind:<4}"), Style::new().fg(Color::DarkGray)),
        Span::raw(format!(
            " {:>5.1}nm {:03.0}° {alt:>7} ",
            t.distance_nm, t.bearing_from_home
        )),
        Span::styled(format!("{arrow} "), Style::new().fg(arrow_color)),
        Span::raw(cpa),
    ])
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
            "↑/↓ select · Esc clear · q quit",
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
            position: pos,
            altitude_ft: Some(30_000.0),
            groundspeed_kt: Some(gs),
            track_deg: Some(track),
            reported_age: Duration::ZERO,
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
}
