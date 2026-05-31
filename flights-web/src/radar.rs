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

use flights_api::{Flight, Meta, PictureResponse};

use crate::display::*;

const FONT_HOME: &str = "bold 18px ui-monospace, SFMono-Regular, Menlo, monospace";
const FONT_LABEL: &str = "12px ui-monospace, SFMono-Regular, Menlo, monospace";

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
        let Some(radius) = meta.get().map(|m| m.radius_nm) else {
            return; // No Search radius yet (pre-/meta): nothing to scale against.
        };
        let pic = picture.get();
        let tracks: Vec<Flight> = pic
            .as_ref()
            .map(|p| p.tracks.clone())
            .unwrap_or_default();
        let pacing = pic.as_ref().and_then(|p| p.pacing_hex.clone());
        let nearest = tracks.first().map(|f| f.hex.clone());
        let sel = selected.get();

        draw(
            &canvas,
            radius,
            &tracks,
            nearest.as_deref(),
            pacing.as_deref(),
            sel.as_deref(),
        );
    });

    view! {
        <div class="panel radar-pane">
            <div class="panel-title">"radar · north-up"</div>
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

/// Paint one frame. Sizes the backing store to the element's CSS box times the
/// device pixel ratio (so blips stay crisp on HiDPI), then draws in CSS-pixel
/// coordinates. Setting `width`/`height` also clears the canvas and resets the
/// transform, so each call starts from a clean slate.
fn draw(
    canvas: &HtmlCanvasElement,
    radius: f64,
    tracks: &[Flight],
    nearest: Option<&str>,
    pacing: Option<&str>,
    selected: Option<&str>,
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

    // Range rings (thirds of the radius) and faint cardinal cross-hairs.
    ctx.set_line_width(1.0);
    ctx.set_stroke_style_str(COL_RING);
    for frac in [1.0 / 3.0, 2.0 / 3.0, 1.0] {
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, radius * frac * scale, 0.0, std::f64::consts::TAU);
        ctx.stroke();
    }
    ctx.set_stroke_style_str(COL_CROSS);
    ctx.begin_path();
    ctx.move_to(EDGE_MARGIN, cy);
    ctx.line_to(css_w - EDGE_MARGIN, cy);
    ctx.move_to(cx, EDGE_MARGIN);
    ctx.line_to(cx, css_h - EDGE_MARGIN);
    ctx.stroke();

    // Blips with heading vectors. North (bearing 0) is up, so the north offset
    // subtracts from the canvas y (which grows downward).
    for t in tracks {
        let (east, north) = radar_xy(t);
        let px = cx + east * scale;
        let py = cy - north * scale;
        let color = blip_color(t, nearest, pacing, selected);

        if let Some((track, gs)) = velocity(t) {
            // Vector length scales with groundspeed, clamped — mirrors the TUI.
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
            3.5
        } else {
            2.5
        };
        ctx.set_fill_style_str(color);
        ctx.begin_path();
        let _ = ctx.arc(px, py, r, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }

    // Home at the center.
    ctx.set_fill_style_str(COL_HOME);
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.set_font(FONT_HOME);
    let _ = ctx.fill_text("⌂", cx, cy);

    // Labels only on flagged flights (nearest / pacing / selected), so the scope
    // doesn't turn into a wall of text.
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
        ctx.set_fill_style_str(blip_color(t, nearest, pacing, selected));
        let _ = ctx.fill_text(&label(t), px + 6.0, py);
    }
}
