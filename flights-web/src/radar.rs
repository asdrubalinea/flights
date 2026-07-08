//! The radar pane: a north-up `<canvas>` that *projects* the Server's Picture onto
//! pixels. Range rings, cardinal cross-hairs, Home at the center, and a blip (with
//! a heading vector) per Track at its `(distance_nm, bearing_deg)` from Home. The
//! canvas only draws answers the Server asserted — it never tweens between polls
//! and never derives a position; smoothness is the poll rate (ADR-0007). It
//! redraws inside an [`Effect`] that tracks the `picture`, `meta`, and `selected`
//! signals, so a new poll or a selection change repaints exactly the changed frame.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use flights_api::{Flight, Health, Meta, PictureResponse};

use crate::display::*;

const FONT_HOME: &str = "bold 18px 'B612 Mono', ui-monospace, monospace";
const FONT_LABEL: &str = "12px 'B612 Mono', ui-monospace, monospace";

/// Empty margin (CSS px) between the outermost range ring and the canvas edge.
const EDGE_MARGIN: f64 = 18.0;

#[component]
pub fn Radar(
    meta: RwSignal<Option<Meta>>,
    picture: RwSignal<Option<PictureResponse>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // A window resize changes none of the data signals, so without this the canvas
    // would keep its old backing-store size — the browser stretching the last frame
    // — until the next poll repaints. Bumping a tick on resize makes the draw Effect
    // below re-run at once at the new size. The listener lives for the whole app, so
    // leaking the handle is the clearest statement of "this is permanent" (mirrors
    // the keyboard listener in `app.rs`).
    let resize_tick = RwSignal::new(0u32);
    let resize = window_event_listener(leptos::ev::resize, move |_| {
        resize_tick.update(|n| *n = n.wrapping_add(1));
    });
    std::mem::forget(resize);

    Effect::new(move |_| {
        // Reading these tracks them: a new Picture, a moved selection, or a window
        // resize repaints.
        let _ = resize_tick.get();
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let Some(m) = meta.get() else {
            return; // No /meta yet: nothing to scale against, no source to show.
        };
        let radius = m.radius_nm;
        let pic = picture.get();
        let tracks: Vec<Flight> = pic
            .as_ref()
            .map(|p| p.tracks.clone())
            .unwrap_or_default();
        let pacing = pic.as_ref().and_then(|p| p.pacing_hex.clone());
        let health = pic.as_ref().map(|p| p.health);
        let nearest = tracks.first().map(|f| f.hex.clone());
        let sel = selected.get();

        draw(
            &canvas,
            radius,
            &tracks,
            nearest.as_deref(),
            pacing.as_deref(),
            sel.as_deref(),
            &m.source,
            health,
        );
    });

    view! {
        <div class="panel radar-pane">
            <div class="panel-title">"nd · north-up"</div>
            <canvas node_ref=canvas_ref class="radar-canvas"></canvas>
        </div>
    }
}

/// Acquire the 2D context, returning `None` if the browser declines (it never
/// does for `"2d"`, but the API is fallible).
fn context2d(canvas: &HtmlCanvasElement) -> Option<CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()??
        .dyn_into::<CanvasRenderingContext2d>()
        .ok()
}

/// Paint one frame as an Airbus-style Navigation Display (ADR-0009). Sizes the
/// backing store to the element's CSS box times the device pixel ratio (so symbols
/// stay crisp on HiDPI), then draws in CSS-pixel coordinates. Setting `width`/
/// `height` also clears the canvas and resets the transform, so each call starts
/// from a clean slate. Layers, back to front: compass rose, traffic (TCAS symbols +
/// heading vectors), Home, call-out labels, and the corner data blocks.
#[allow(clippy::too_many_arguments)]
fn draw(
    canvas: &HtmlCanvasElement,
    radius: f64,
    tracks: &[Flight],
    nearest: Option<&str>,
    pacing: Option<&str>,
    selected: Option<&str>,
    source: &str,
    health: Option<Health>,
) {
    let css_w = canvas.client_width() as f64;
    let css_h = canvas.client_height() as f64;
    if css_w < 1.0 || css_h < 1.0 {
        return; // Not laid out yet; the next poll repaints once it has a size.
    }
    let dpr = window().device_pixel_ratio().max(1.0);
    canvas.set_width((css_w * dpr) as u32);
    canvas.set_height((css_h * dpr) as u32);

    let Some(ctx) = context2d(canvas) else {
        return;
    };
    let _ = ctx.scale(dpr, dpr); // Draw in CSS px; the backing store is denser.

    let (cx, cy) = (css_w / 2.0, css_h / 2.0);
    // Pixels per nm: the shorter half-axis (minus a margin) carries the full
    // radius, so the outermost ring just fits whatever the pane's shape is.
    let scale = (css_w.min(css_h) / 2.0 - EDGE_MARGIN) / radius;
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }

    draw_rose(&ctx, cx, cy, radius, scale, css_w, css_h);

    // Traffic: a TCAS symbol + heading vector per track. North (bearing 0) is up, so
    // the north offset subtracts from the canvas y (which grows downward).
    for t in tracks {
        let (east, north) = radar_xy(t);
        let px = cx + east * scale;
        let py = cy - north * scale;
        draw_track(&ctx, t, px, py, scale, radius, nearest, pacing, selected);
    }

    // Home at the center (white = reference, ADR-0009).
    ctx.set_fill_style_str(COL_HOME);
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.set_font(FONT_HOME);
    let _ = ctx.fill_text("⌂", cx, cy);

    // Labels only on flagged flights (nearest / pacing / selected / emergency), so the
    // scope doesn't turn into a wall of text.
    ctx.set_text_align("left");
    ctx.set_text_baseline("middle");
    ctx.set_font(FONT_LABEL);
    for t in tracks {
        if !is_flagged(t, nearest, pacing, selected) {
            continue;
        }
        let (east, north) = radar_xy(t);
        let px = cx + east * scale;
        let py = cy - north * scale;
        ctx.set_fill_style_str(symbol_color(t, nearest, pacing));
        let _ = ctx.fill_text(&label(t), px + 8.0, py);
    }

    draw_corner_data(&ctx, css_w, css_h, tracks.len(), radius, source, health);
}

/// The compass rose: faint cross-hairs, three range rings (thirds of the radius)
/// labelled in nm, a 10°-graduated tick scale (longer ticks every 30°), and the
/// cardinal/decadal heading numbers just inside the outer ring. North-up.
fn draw_rose(
    ctx: &CanvasRenderingContext2d,
    cx: f64,
    cy: f64,
    radius: f64,
    scale: f64,
    css_w: f64,
    css_h: f64,
) {
    use std::f64::consts::TAU;
    let outer = radius * scale;

    // Faint cardinal cross-hairs.
    ctx.set_stroke_style_str(COL_CROSS);
    ctx.set_line_width(1.0);
    ctx.begin_path();
    ctx.move_to(EDGE_MARGIN, cy);
    ctx.line_to(css_w - EDGE_MARGIN, cy);
    ctx.move_to(cx, EDGE_MARGIN);
    ctx.line_to(cx, css_h - EDGE_MARGIN);
    ctx.stroke();

    // Range rings + their nm labels (read up the north axis).
    ctx.set_stroke_style_str(COL_RING);
    ctx.set_line_width(1.0);
    ctx.set_font(FONT_LABEL);
    ctx.set_fill_style_str(COL_ANON);
    ctx.set_text_align("left");
    ctx.set_text_baseline("middle");
    for frac in [1.0 / 3.0, 2.0 / 3.0, 1.0] {
        let rr = radius * frac * scale;
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, rr, 0.0, TAU);
        ctx.stroke();
        let _ = ctx.fill_text(&format!("{:.0}", radius * frac), cx + 4.0, cy - rr + 8.0);
    }

    // 10° graduated ticks around the outer ring (longer every 30°).
    ctx.set_stroke_style_str(COL_RING);
    for deg in (0..360).step_by(10) {
        let a = (deg as f64).to_radians();
        let (sx, sy) = (a.sin(), -a.cos()); // north-up unit vector
        let inner = outer - if deg % 30 == 0 { 9.0 } else { 5.0 };
        ctx.set_line_width(1.0);
        ctx.begin_path();
        ctx.move_to(cx + sx * inner, cy + sy * inner);
        ctx.line_to(cx + sx * outer, cy + sy * outer);
        ctx.stroke();
    }

    // Cardinal + decadal heading labels (N / E / S / W and 3, 6, 12 …), white.
    ctx.set_fill_style_str(COL_HOME);
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    for deg in (0..360).step_by(30) {
        let a = (deg as f64).to_radians();
        let (sx, sy) = (a.sin(), -a.cos());
        let rr = outer - 17.0;
        let txt = match deg {
            0 => "N".to_string(),
            90 => "E".to_string(),
            180 => "S".to_string(),
            270 => "W".to_string(),
            _ => format!("{}", deg / 10),
        };
        let _ = ctx.fill_text(&txt, cx + sx * rr, cy + sy * rr);
    }
}

/// One track: its heading vector, its TCAS symbol (filled diamond = in-contact,
/// hollow = lost, square = emergency), the nearest-flight ring, and the white
/// selection box. Colour comes from `symbol_color`; selection is orthogonal to it.
#[allow(clippy::too_many_arguments)]
fn draw_track(
    ctx: &CanvasRenderingContext2d,
    t: &Flight,
    px: f64,
    py: f64,
    scale: f64,
    radius: f64,
    nearest: Option<&str>,
    pacing: Option<&str>,
    selected: Option<&str>,
) {
    let color = symbol_color(t, nearest, pacing);

    // Heading vector — length scales with groundspeed, clamped (mirrors the TUI).
    if let Some((track, gs)) = velocity(t) {
        let len_nm = (radius * 0.05 * (gs / 300.0)).clamp(radius * 0.02, radius * 0.12);
        let a = track.to_radians();
        ctx.set_stroke_style_str(color);
        ctx.set_line_width(1.5);
        ctx.begin_path();
        ctx.move_to(px, py);
        ctx.line_to(px + a.sin() * len_nm * scale, py - a.cos() * len_nm * scale);
        ctx.stroke();
    }

    let r = if is_flagged(t, nearest, pacing, selected) {
        5.0
    } else {
        4.0
    };
    match symbol_shape(t.state, t.emergency.as_deref()) {
        Shape::Diamond => {
            diamond_path(ctx, px, py, r);
            ctx.set_fill_style_str(color);
            ctx.fill();
        }
        Shape::OpenDiamond => {
            diamond_path(ctx, px, py, r);
            ctx.set_stroke_style_str(color);
            ctx.set_line_width(1.5);
            ctx.stroke();
        }
        Shape::Square => {
            ctx.set_fill_style_str(color);
            ctx.fill_rect(px - r, py - r, 2.0 * r, 2.0 * r);
        }
    }

    // Nearest flight gets a ring so it reads apart from the green approach field.
    if nearest == Some(t.hex.as_str()) {
        ctx.set_stroke_style_str(color);
        ctx.set_line_width(1.0);
        ctx.begin_path();
        let _ = ctx.arc(px, py, r + 4.0, 0.0, std::f64::consts::TAU);
        ctx.stroke();
    }

    // Selection: an orthogonal white box, so a selected emergency still reads red.
    if selected == Some(t.hex.as_str()) {
        let s = r + 4.0;
        ctx.set_stroke_style_str(COL_SELECTED);
        ctx.set_line_width(1.0);
        ctx.stroke_rect(px - s, py - s, 2.0 * s, 2.0 * s);
    }
}

/// Trace a diamond (rotated square) path centred at `(px, py)` with half-diagonal
/// `r`; the caller fills or strokes it.
fn diamond_path(ctx: &CanvasRenderingContext2d, px: f64, py: f64, r: f64) {
    ctx.begin_path();
    ctx.move_to(px, py - r);
    ctx.line_to(px + r, py);
    ctx.line_to(px, py + r);
    ctx.line_to(px - r, py);
    ctx.close_path();
}

/// The four corner data blocks, ND-style: traffic count and range across the top,
/// source and Picture health across the bottom (health in its status colour).
fn draw_corner_data(
    ctx: &CanvasRenderingContext2d,
    css_w: f64,
    css_h: f64,
    count: usize,
    radius: f64,
    source: &str,
    health: Option<Health>,
) {
    let (htxt, hcol) = match health {
        Some(Health::Live) => ("LIVE", COL_APPROACH),
        Some(Health::Stale) => ("STALE", COL_CAUTION),
        Some(Health::NoData) | None => ("NO DATA", COL_EMERGENCY),
    };
    ctx.set_font(FONT_LABEL);
    ctx.set_text_baseline("middle");
    let (top, bot) = (12.0, css_h - 12.0);

    ctx.set_fill_style_str(COL_ANON);
    ctx.set_text_align("left");
    let _ = ctx.fill_text(&format!("TFC {count}"), EDGE_MARGIN, top);
    let _ = ctx.fill_text(&format!("SRC {source}"), EDGE_MARGIN, bot);

    ctx.set_text_align("right");
    let _ = ctx.fill_text(&format!("RNG {radius:.0}nm"), css_w - EDGE_MARGIN, top);
    ctx.set_fill_style_str(hcol);
    let _ = ctx.fill_text(htxt, css_w - EDGE_MARGIN, bot);
}
