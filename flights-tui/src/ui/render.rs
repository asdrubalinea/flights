//! Drawing. One frame = a north-up radar canvas on the left, a flight list and a
//! status block on the right. Everything is derived from a single
//! [`flights_api::PictureResponse`] the Server computed, so the radar, list, and
//! status never disagree — the Client only renders, it never recomputes geometry
//! (ADR-0005). The detail popup renders the lazily-fetched
//! [`flights_api::FlightDetail`], whose opaque groups are shown verbatim.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use flights_api::{ContactState, Cpa, DetailGroup, Flight, FlightDetail, VerticalTrend};

use super::app::{App, Conn, DetailView, Mode};

const RING_FRACTIONS: [f64; 3] = [1.0 / 3.0, 2.0 / 3.0, 1.0];

/// Terminal character cells are roughly twice as tall as they are wide. The
/// radar corrects for this so equal range means equal screen distance from Home
/// in every direction — otherwise the range rings render as ovals.
const CELL_ASPECT: f64 = 2.0;

/// Groundspeed below this (knots) is treated as stationary: no heading vector.
/// Mirrors the Server's domain floor — purely a display threshold here.
const MOVING_FLOOR_KT: f64 = 1.0;

pub fn draw(frame: &mut Frame, app: &mut App) {
    // The Server already produced one self-consistent picture; we just render it.
    let tracks: Vec<Flight> = app.tracks().to_vec();
    let nearest_hex = tracks.first().map(|f| f.hex.clone());
    let pacing_hex = app.picture.as_ref().and_then(|p| p.pacing_hex.clone());
    let radius = app.meta.radius_nm;

    let [radar_area, panel] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .areas(frame.area());
    let [list_area, status_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(8)]).areas(panel);

    draw_radar(
        frame,
        radar_area,
        radius,
        &tracks,
        nearest_hex.clone(),
        pacing_hex.clone(),
        app.selected_hex.clone(),
    );
    draw_list(
        frame,
        list_area,
        &tracks,
        pacing_hex.as_deref(),
        app.selected_hex.as_deref(),
        &mut app.list_state,
    );
    draw_status(
        frame,
        status_area,
        app,
        &tracks,
        nearest_hex.as_deref(),
        pacing_hex.as_deref(),
        app.selected_hex.as_deref(),
    );

    // The flight-detail popup layers over the dimmed radar+list.
    if app.mode == Mode::Detail {
        let detail = app.detail.clone();
        // The popup body is a possibly-older /flight fetch; the stale banner is
        // derived from the *live* picture so a flight that goes lost while open is
        // marked at once, without a re-fetch (ADR-0007: last-known detail survives).
        let banner = app
            .selected_hex
            .as_deref()
            .and_then(|hex| tracks.iter().find(|f| f.hex == hex))
            .and_then(lost_banner);
        draw_detail(frame, detail.as_ref(), banner, &mut app.detail_scroll);
    }
}

fn draw_radar(
    frame: &mut Frame,
    area: Rect,
    radius: f64,
    tracks: &[Flight],
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

    let tracks = tracks.to_vec();
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
            for t in &tracks {
                let (x, y) = radar_xy(t);
                let color = blip_color(t, &nearest_hex, &pacing_hex, &selected_hex);
                if let Some((track, gs)) = velocity(t) {
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
            for t in &tracks {
                if !is_flagged(t, &nearest_hex, &pacing_hex, &selected_hex) {
                    continue;
                }
                let (x, y) = radar_xy(t);
                let color = blip_color(t, &nearest_hex, &pacing_hex, &selected_hex);
                // A lost flight (e.g. a lost Nearest) gets a ✕ so the faded blip
                // reads as lost rather than merely anonymous.
                let text = if is_lost(t) {
                    format!("{} ✕", label(t))
                } else {
                    label(t)
                };
                ctx.print(
                    x + radius * 0.03,
                    y,
                    Line::from(Span::styled(text, Style::new().fg(color))),
                );
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    tracks: &[Flight],
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

    let index = selected_hex.and_then(|hex| tracks.iter().position(|t| t.hex == hex));
    state.select(index);
    frame.render_stateful_widget(list, area, state);
}

fn list_row(t: &Flight, pacing_hex: Option<&str>, max_width: u16) -> Line<'static> {
    let is_pacing = Some(t.hex.as_str()) == pacing_hex;
    let ident_style = if is_pacing {
        Style::new()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD)
    } else if is_lost(t) || t.ident.is_none() {
        // Lost flights fade like anonymous ones (a lost flight never paces).
        Style::new().fg(Color::DarkGray)
    } else {
        Style::new()
    };

    let alt = t
        .altitude_ft
        .map(|a| format!("{a:.0}ft"))
        .unwrap_or_else(|| "?".into());

    // The ICAO type designator (e.g. B738), dimmed so it reads as secondary to
    // the callsign. Blank when the Server didn't supply one.
    let kind = t.aircraft_type.as_deref().unwrap_or("");

    // Climb/descend/level glyph glued to the altitude (the Server derived the trend).
    let (glyph, glyph_color) = trend_glyph(t.vertical_trend);

    let gs_span = match t.groundspeed_kt {
        Some(g) => Span::raw(format!(" {g:>3.0}kt")),
        None => Span::raw(String::new()),
    };

    // A lost flight shows its reason badge here instead of a (stale) CPA — its CPA
    // is frozen and never paces, so the approach arrow would mislead.
    let (arrow, arrow_color, cpa) = match lost_badge(t.state) {
        Some((word, color)) => ("✕", color, word.to_string()),
        None => match &t.cpa {
            Some(c) if approaching(c) => (
                "▲",
                Color::Green,
                format!("{:.0}nm/{:.0}s", c.cpa_distance_nm, c.time_to_cpa_s),
            ),
            Some(_) => ("▼", Color::Blue, String::new()),
            None => ("·", Color::DarkGray, String::new()),
        },
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
            vec![Span::styled(format!("{:<8}", label(t)), ident_style)],
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
                t.distance_nm, t.bearing_deg
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
    tracks: &[Flight],
    nearest_hex: Option<&str>,
    pacing_hex: Option<&str>,
    selected_hex: Option<&str>,
) {
    // The Client's "server unreachable" state takes precedence over the Server's
    // own health (which we can't trust once we can't reach it).
    let (state_label, state_color, status_error) = match &app.conn {
        Conn::Down(e) => ("UNREACHABLE", Color::Red, Some(e.clone())),
        Conn::Ok => match &app.picture {
            Some(p) => match p.health {
                flights_api::Health::Live => ("LIVE", Color::Green, None),
                flights_api::Health::Stale => ("STALE", Color::Yellow, p.last_error.clone()),
                flights_api::Health::NoData => ("NO DATA", Color::Red, p.last_error.clone()),
            },
            None => ("NO DATA", Color::Red, None),
        },
    };

    let age = app
        .picture
        .as_ref()
        .and_then(|p| p.snapshot_age_s)
        .map(|s| format!("{s:.0}s ago"))
        .unwrap_or_else(|| "—".into());

    let find = |hex: Option<&str>| hex.and_then(|h| tracks.iter().find(|t| t.hex == h));
    // The Nearest flight can be one we've lost contact with (still the closest thing
    // we know of) — badge it so "nearest" isn't read as "currently tracked".
    let nearest = find(nearest_hex)
        .map(|f| match lost_badge(f.state) {
            Some((word, _)) => format!("{}  [lost: {word}]", fmt_position(f)),
            None => fmt_position(f),
        })
        .unwrap_or_else(|| "(none)".into());
    let pacing = find(pacing_hex)
        .map(fmt_pacing)
        .unwrap_or_else(|| "quiet — backing off".into());

    // The selected flight's live (Server-dead-reckoned) coordinates and how old
    // the underlying position report now is — useful for cross-referencing.
    let selected = find(selected_hex).map(|t| {
        let model = t
            .model
            .as_deref()
            .or(t.aircraft_type.as_deref())
            .unwrap_or("unknown type");
        format!(
            "sel {} · {model}  {:.3}, {:.3}  ({:.0}s old)",
            label(t),
            t.lat,
            t.lon,
            t.age_s
        )
    });

    let mut lines = vec![
        Line::from(vec![
            Span::raw("source "),
            Span::styled(
                app.meta.source.clone(),
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
        Line::from(format!("relevance {:.0}nm", app.meta.relevance_nm)),
    ];
    match selected {
        Some(s) => lines.push(Line::from(Span::styled(s, Style::new().fg(Color::White)))),
        None => lines.push(Line::from(Span::styled(
            "↑/↓ select · Enter detail · Esc clear · q quit",
            Style::new().fg(Color::DarkGray),
        ))),
    }
    if let Some(err) = status_error {
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

/// The flight-detail popup: dim the frame, clear a centered box, and fill it from
/// the lazily-fetched [`FlightDetail`] — or a "left the area" / error notice when
/// the flight has aged out or the fetch failed (never fabricated values). A `banner`
/// (set when the live picture has the flight as *lost*) is prepended to the loaded
/// body so the last-known detail reads as stale. The key hints sit on a fixed bottom
/// line so scrolling the body never loses them.
fn draw_detail(
    frame: &mut Frame,
    detail: Option<&DetailView>,
    banner: Option<Line<'static>>,
    scroll: &mut u16,
) {
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

    let (body, footer) = match detail {
        Some(DetailView::Loaded(detail)) => {
            let mut lines = detail_lines(detail);
            // Lead with the stale banner when the flight is currently lost.
            if let Some(b) = banner {
                lines.insert(0, b);
                lines.insert(1, Line::from(""));
            }
            (lines, "Esc close · ↑/↓ flight · PgUp/PgDn scroll")
        }
        Some(DetailView::LeftArea) => (
            notice("— flight left the area —"),
            "Esc close · ↑/↓ next flight",
        ),
        Some(DetailView::Error(e)) => (notice(&format!("— {e} —")), "Esc close"),
        None => (notice("— loading… —"), "Esc close"),
    };

    // Clamp so the body can't be scrolled entirely past the viewport, leaving a
    // void below it. The promoted fields are short, but an opaque adapter detail
    // value can be long and wrap, so we bound against the *rendered* row count (how
    // many rows the body occupies once word-wrapped) rather than the logical line
    // count — otherwise the bottom of a wrapped popup stays unreachable.
    let rendered_rows: u16 = body
        .iter()
        .map(|line| wrapped_rows(line, body_area.width))
        .fold(0u16, |acc, r| acc.saturating_add(r));
    let max_scroll = rendered_rows.saturating_sub(body_area.height);
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

/// A two-line centered placeholder body for the popup's non-loaded states.
fn notice(text: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {text}"),
            Style::new().fg(Color::DarkGray),
        )),
    ]
}

/// Every displayable field for one flight: a header, the promoted typed fields
/// grouped under "Position & motion" / "Transponder", then the Server-contributed
/// [`DetailGroup`]s rendered generically (no per-Source code).
fn detail_lines(detail: &FlightDetail) -> Vec<Line<'static>> {
    let f = &detail.flight;
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
    lines.push(field_line("Distance", &format!("{:.1} nm", f.distance_nm)));
    lines.push(field_line("Bearing", &format!("{:03.0}°", f.bearing_deg)));
    if let Some(a) = f.altitude_ft {
        lines.push(field_line("Baro alt", &format!("{a:.0} ft")));
    }
    if let Some(a) = f.geometric_altitude_ft {
        lines.push(field_line("Geo alt", &format!("{a:.0} ft")));
    }
    if let Some(r) = f.vertical_rate_fpm {
        let (glyph, _) = trend_glyph(f.vertical_trend);
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
        &format!("{:.4}, {:.4}", f.lat, f.lon),
    ));
    lines.push(field_line("Position age", &format!("{:.0}s", f.age_s)));

    if f.squawk.is_some() || f.emergency.is_some() {
        section_title(&mut lines, "Transponder");
        if let Some(s) = &f.squawk {
            lines.push(field_line("Squawk", s));
        }
        if let Some(e) = &f.emergency {
            lines.push(field_line("Emergency", e));
        }
    }

    // The opaque, Server-formatted detail groups — rendered verbatim.
    for group in &detail.details {
        render_group(&mut lines, group);
    }

    lines
}

fn render_group(lines: &mut Vec<Line<'static>>, group: &DetailGroup) {
    section_title(lines, &group.title);
    for field in &group.fields {
        lines.push(field_line(&field.label, &field.value));
    }
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
        Span::styled(format!("  {label:<16}"), Style::new().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

/// How many terminal rows a logical line occupies once word-wrapped to `width`
/// (as the popup body's `Wrap { trim: false }` renders it). A greedy model: words
/// are separated by single spaces, an over-long word is hard-broken across rows.
/// A line that fits returns 1, so for the common (non-wrapping) popup this equals
/// the logical line count — it only diverges when an opaque detail value is long.
fn wrapped_rows(line: &Line, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    // Rows added, and the end column, when a word is laid out from column 0 —
    // hard-breaking it across rows when it is wider than the line.
    let from_start = |wlen: usize| -> (usize, usize) {
        if wlen == 0 {
            (0, 0)
        } else {
            ((wlen - 1) / width, (wlen - 1) % width + 1)
        }
    };

    let mut rows: usize = 1;
    let mut col: usize = 0;
    for word in text.split(' ') {
        let wlen = word.chars().count();
        if col == 0 {
            let (extra, end) = from_start(wlen);
            rows += extra;
            col = end;
        } else if col + 1 + wlen <= width {
            // Fits on the current row after a separating space.
            col += 1 + wlen;
        } else {
            // Wrap to a fresh row, then lay the word out from column 0.
            rows += 1;
            let (extra, end) = from_start(wlen);
            rows += extra;
            col = end;
        }
    }
    rows as u16
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

/// A display label: the callsign, or the bracketed hex for an ident-blocked flight.
fn label(f: &Flight) -> String {
    f.ident.clone().unwrap_or_else(|| format!("[{}]", f.hex))
}

/// A flight's velocity as `(track_deg, groundspeed_kt)` when usably moving, for the
/// radar heading vector. `None` ⇒ no vector drawn.
fn velocity(f: &Flight) -> Option<(f64, f64)> {
    match (f.track_deg, f.groundspeed_kt) {
        (Some(track), Some(gs)) if gs >= MOVING_FLOOR_KT => Some((track, gs)),
        _ => None,
    }
}

/// A flight whose closest pass is still ahead of it (the Server signs it via
/// `time_to_cpa_s`).
fn approaching(c: &Cpa) -> bool {
    c.time_to_cpa_s >= 0.0
}

/// Whether the Server has **lost contact** with this flight (any reason). A lost
/// flight is frozen at its last-known position and rendered faded, not dropped.
fn is_lost(f: &Flight) -> bool {
    f.state != ContactState::InContact
}

/// A compact badge for a lost flight: a short reason word and a muted colour, or
/// `None` while in contact. Colours stay subdued so a one-poll dropout fades rather
/// than flashing an alarm (ADR-0007).
fn lost_badge(state: ContactState) -> Option<(&'static str, Color)> {
    match state {
        ContactState::InContact => None,
        ContactState::Landed => Some(("landed", Color::DarkGray)),
        ContactState::LeftScope => Some(("left scope", Color::DarkGray)),
        // The honest residual — worth a glance, but still muted, not alarming.
        ContactState::LostContact => Some(("lost", Color::Yellow)),
    }
}

/// A one-line stale banner for the detail popup when the selected flight is lost,
/// reflecting the *live* picture (the popup body is a possibly-older fetch). `None`
/// while in contact.
fn lost_banner(f: &Flight) -> Option<Line<'static>> {
    let (text, color) = match f.state {
        ContactState::InContact => return None,
        ContactState::Landed => ("landed — last-known data", Color::DarkGray),
        ContactState::LeftScope => ("left the Search area — last-known data", Color::DarkGray),
        ContactState::LostContact => ("contact lost — last-known data", Color::Yellow),
    };
    Some(Line::from(Span::styled(
        format!("⚠ {text}, {:.0}s ago", f.age_s),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )))
}

fn fmt_position(f: &Flight) -> String {
    let alt = f
        .altitude_ft
        .map(|a| format!("{a:.0} ft"))
        .unwrap_or_else(|| "alt ?".into());
    let kind = f
        .aircraft_type
        .as_deref()
        .map(|k| format!(" {k}"))
        .unwrap_or_default();
    format!(
        "{}{kind} — {:.1} nm @ {:03.0}° ({alt})",
        label(f),
        f.distance_nm,
        f.bearing_deg
    )
}

fn fmt_pacing(f: &Flight) -> String {
    match &f.cpa {
        Some(c) => format!(
            "{} — CPA {:.1} nm in {:.0}s",
            label(f),
            c.cpa_distance_nm,
            c.time_to_cpa_s
        ),
        None => label(f),
    }
}

/// A flight's radar position as (east, north) nm offsets from Home.
fn radar_xy(f: &Flight) -> (f64, f64) {
    let b = f.bearing_deg.to_radians();
    (f.distance_nm * b.sin(), f.distance_nm * b.cos())
}

fn is_flagged(
    f: &Flight,
    nearest: &Option<String>,
    pacing: &Option<String>,
    selected: &Option<String>,
) -> bool {
    let hex = Some(f.hex.clone());
    hex == *nearest || hex == *pacing || hex == *selected
}

/// Blip color by priority: selected, then *lost* (faded), then pacing, then
/// nearest, then approach state. A lost flight fades to grey even when it is the
/// Nearest — selection still wins, so the user can see what they picked. Anonymous
/// (ident-blocked) flights are dimmed unless flagged.
fn blip_color(
    f: &Flight,
    nearest: &Option<String>,
    pacing: &Option<String>,
    selected: &Option<String>,
) -> Color {
    let hex = f.hex.as_str();
    if selected.as_deref() == Some(hex) {
        return Color::White;
    }
    if is_lost(f) {
        return Color::DarkGray;
    }
    if pacing.as_deref() == Some(hex) {
        return Color::LightRed;
    }
    if nearest.as_deref() == Some(hex) {
        return Color::Cyan;
    }
    if f.ident.is_none() {
        return Color::DarkGray;
    }
    match &f.cpa {
        Some(c) if approaching(c) => Color::Green,
        _ => Color::Blue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use crate::ui::app::{App, DetailView, Mode};
    use flights_api::{
        ContactState, Cpa, DetailField, DetailGroup, Flight, FlightDetail, Health, LatLon, Meta,
        PictureResponse, Units, VerticalTrend,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::Duration;

    fn meta() -> Meta {
        Meta {
            home: LatLon { lat: 0.0, lon: 0.0 },
            radius_nm: 100.0,
            relevance_nm: 30.0,
            source: "testsrc".into(),
            units: Units::aviation(),
            version: "test".into(),
        }
    }

    fn flight(hex: &str, ident: &str, distance_nm: f64, bearing_deg: f64) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some(ident.into()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: None,
            operator: None,
            lat: 0.05,
            lon: 0.0,
            distance_nm,
            bearing_deg,
            altitude_ft: Some(30_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(452.0),
            track_deg: Some(90.0),
            vertical_rate_fpm: Some(1200.0),
            vertical_trend: VerticalTrend::Climb,
            squawk: None,
            emergency: None,
            emitter_category: None,
            state: ContactState::InContact,
            age_s: 0.0,
            cpa: Some(Cpa {
                time_to_cpa_s: 120.0,
                cpa_distance_nm: 2.0,
            }),
        }
    }

    fn app(tracks: Vec<Flight>, pacing_hex: Option<String>) -> App {
        let mut a = App::new(
            Client::new("http://127.0.0.1:0"),
            meta(),
            Duration::from_millis(250),
        );
        a.picture = Some(PictureResponse {
            as_of: 0.0,
            health: Health::Live,
            last_error: None,
            snapshot_age_s: Some(1.0),
            pacing_hex,
            tracks,
        });
        a
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

    #[test]
    fn draws_radar_list_and_status_with_nearest_and_pacing() {
        let overhead = flight("aaa111", "OVERHEAD", 3.0, 0.0);
        let inbound = flight("bbb222", "INBOUND", 24.0, 180.0);
        let mut app = app(vec![overhead, inbound], Some("bbb222".into()));

        let text = render_to_text(&mut app, 120, 40);
        for needle in [
            "radar", "flights", "status", "testsrc", "LIVE", "nearest", "pacing", "OVERHEAD",
            "INBOUND", "select", "B738",
        ] {
            assert!(text.contains(needle), "rendered frame missing {needle:?}");
        }
    }

    #[test]
    fn shows_server_unreachable_state() {
        let mut app = app(vec![flight("aaa111", "ALPHA", 3.0, 0.0)], None);
        app.conn = super::Conn::Down("connection refused".into());
        let text = render_to_text(&mut app, 120, 40);
        assert!(
            text.contains("UNREACHABLE"),
            "expected the unreachable state"
        );
        assert!(
            text.contains("connection refused"),
            "expected the error line"
        );
    }

    #[test]
    fn popup_paints_detail_in_detail_mode() {
        let mut app = app(vec![flight("sel123", "SELFLT", 3.0, 90.0)], None);
        app.selected_hex = Some("sel123".into());
        app.mode = Mode::Detail;
        let mut f = flight("sel123", "SELFLT", 3.0, 90.0);
        f.registration = Some("N12345".into());
        app.detail = Some(DetailView::Loaded(Box::new(FlightDetail {
            flight: f,
            details: vec![DetailGroup {
                title: "Signal".into(),
                fields: vec![DetailField {
                    label: "RSSI".into(),
                    value: "-7.4 dBFS".into(),
                }],
            }],
        })));

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
    fn popup_shows_left_the_area() {
        let mut app = app(vec![flight("present", "PRESENT", 3.0, 0.0)], None);
        app.selected_hex = Some("ghost".into());
        app.mode = Mode::Detail;
        app.detail = Some(DetailView::LeftArea);

        let text = render_to_text(&mut app, 120, 40);
        assert!(
            text.contains("flight left the area"),
            "expected the left-the-area notice"
        );
    }

    #[test]
    fn list_row_badges_a_lost_flight_instead_of_a_stale_cpa() {
        // flight() carries an approaching CPA; once lost, the reason badge replaces
        // it (a frozen CPA must not read as a live approach).
        let mut f = flight("lost01", "GHOST", 5.0, 90.0);
        f.state = ContactState::LeftScope;
        let line = list_row(&f, None, 60);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("left scope"), "row should badge the reason: {text:?}");
        assert!(!text.contains("nm/"), "a lost flight shows no live CPA: {text:?}");
    }

    #[test]
    fn a_lost_nearest_is_badged_in_the_status_line() {
        let mut lost = flight("lost01", "GH", 5.0, 90.0);
        lost.state = ContactState::LeftScope;
        let mut app = app(vec![lost], None);
        // A wide terminal so the badge isn't clipped off the status line.
        let text = render_to_text(&mut app, 200, 40);
        assert!(
            text.contains("[lost: left scope]"),
            "the nearest line should badge a lost flight"
        );
    }

    #[test]
    fn popup_shows_a_stale_banner_for_a_lost_selected_flight() {
        // The selected flight is lost in the live picture; the popup keeps its
        // last-known detail but leads with a stale banner (ADR-0007).
        let mut lost = flight("sel123", "SELFLT", 3.0, 90.0);
        lost.state = ContactState::LostContact;
        lost.age_s = 45.0;
        let mut app = app(vec![lost.clone()], None);
        app.selected_hex = Some("sel123".into());
        app.mode = Mode::Detail;
        app.detail = Some(DetailView::Loaded(Box::new(FlightDetail {
            flight: lost,
            details: vec![],
        })));

        let text = render_to_text(&mut app, 120, 40);
        assert!(
            text.contains("contact lost"),
            "popup should show the stale banner: {text:?}"
        );
        assert!(
            text.contains("SELFLT"),
            "the last-known detail still shows beneath the banner"
        );
    }

    #[test]
    fn list_row_survives_a_narrow_terminal() {
        let f = flight("aaa111", "TIGHTONE", 3.0, 90.0);
        let line = list_row(&f, None, 16);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("TIGHTONE"), "callsign should survive");
    }

    #[test]
    fn list_row_drops_altitude_and_its_trend_glyph_together() {
        // A climbing flight (↑) at 30000 ft. Altitude and its glyph share one fit
        // group, so at a width that can't hold the altitude the glyph must not
        // survive alone.
        let f = flight("aaa111", "TIGHT", 3.0, 90.0);
        let line = list_row(&f, None, 22);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("TIGHT"), "callsign survives");
        assert!(!text.contains("30000"), "altitude is dropped at this width");
        assert!(
            !text.contains('↑'),
            "trend glyph must not survive without its altitude"
        );
    }

    #[test]
    fn wrapped_rows_counts_wrap_for_long_lines() {
        // A line that fits is one row (the common popup case).
        assert_eq!(wrapped_rows(&Line::from("short value"), 40), 1);
        assert_eq!(wrapped_rows(&Line::from(""), 40), 1);
        // A value longer than the width wraps across rows.
        assert_eq!(wrapped_rows(&Line::from("a".repeat(80)), 40), 2);
        assert_eq!(wrapped_rows(&Line::from("a".repeat(81)), 40), 3);
        // Word-boundary wrap: two words that don't share a row.
        let twenty = "x".repeat(20);
        assert_eq!(
            wrapped_rows(&Line::from(format!("{twenty} {twenty}")), 25),
            2
        );
    }

    #[test]
    fn fit_groups_drops_whole_groups_keeping_priority_order() {
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
