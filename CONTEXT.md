# Flights

A long-running app that tracks the **nearest flight** to a fixed **Home**
location using a pluggable **Source** (initially the free, community-run
airplanes.live ADS-B API), deciding *itself* how often to poll so it stays within
each Source's limits and keeps the screen smooth. The current build is a radar-style
TUI — a Home-centered scope showing nearby flights as gliding blips, alongside a
list of those flights — that you watch directly; the intended eventual use is to
feed a waybar status-bar widget.

## Language

**Nearest flight**:
The airborne flight whose last reported position is the smallest great-circle
*ground* (horizontal) distance from Home. Altitude is not part of the measure —
a plane directly overhead counts as essentially zero distance away.
_Avoid_: "closest plane", "overhead flight" (those imply altitude/slant range).

**Home**:
The single fixed geographic point (latitude/longitude) that all distances are
measured from. It does not change at runtime.
_Avoid_: "current location", "my position" (those imply dynamic/device location,
which this is deliberately not).

**Search area**:
The region around Home we ask a Source about, expressed in the domain as Home
plus a **radius**. Any flight outside it is invisible to the app, even one just
beyond the edge that is closer than something inside. Sources that only accept a
bounding box (e.g. OpenSky) convert the radius to a box and filter back to the
radius *inside their adapter* — the poller, tracker, and UI only ever think in
radius.
_Avoid_: "bounding box" as a domain term (it is an adapter-level detail for
box-only sources, not something the rest of the app knows about).

**Closest point of approach (CPA)**:
For a flight holding its current course and speed, the moment it will be nearest
to Home — described by a *time* (how soon) and a *distance* (how close it will
get). Derived from the flight's reported position, heading, and groundspeed, and
re-estimated on every poll.

**Approaching flight**:
A flight whose CPA is still in the future — it is currently closing on Home, as
opposed to a *receding* flight whose closest pass is already behind it.
_Avoid_: "incoming" (we track geometry, not intent).

**Relevance distance**:
The CPA-distance cutoff beyond which an approaching flight is ignored for pacing.
A flight that will only ever miss Home by a wide margin never speeds up polling,
no matter how soon it passes. Bounded by the Search area — we cannot pace on what
we cannot see.
_Note_: this gates *pacing only*, never the display. The radar and list always
show every flight in the Search area; "relevant" never means "the only ones shown."

**Pacing flight**:
Among approaching flights whose CPA distance is within the Relevance distance,
the one whose CPA is *soonest*. It sets the current poll cadence. Distinct from
the **Nearest flight**: the Nearest flight is what the display shows; the Pacing
flight is what decides when we next spend an API call. They are often different
aircraft (e.g. one just passed and is receding while another is inbound).

**Dead reckoning**:
Estimating a flight's present position *between* polls by extrapolating its last
reported position along its last reported velocity, so the display stays current
without spending an API call. Polling exists to correct the drift this
accumulates and to discover new flights — not to refresh the screen.

**Snapshot**:
The complete set of flights in the Search area returned by a single poll,
authoritative as of its timestamp. A new Snapshot replaces the previous one
wholesale — flights it omits have left the box. Between Snapshots the app
dead-reckons the latest one; if polls stop arriving, the current Snapshot is held
and flagged stale, and individual flights are dropped once they age past a
staleness cap so the radar never shows fiction indefinitely.

**Source** (data source):
A pluggable provider of flight Snapshots — a free ADS-B API (airplanes.live today;
adsb.lol, adsb.fi, and a local dump1090/readsb box all speak the same shape) or a
paid API later. Every Source sits behind one interface that takes a Search area
and returns a Snapshot of domain flights, so the poller, tracker, and UI never
know which is active. Each Source declares its own minimum poll interval — a local
receiver can be polled far faster than a rate-limited public API.

**Aircraft type** / **model**:
What kind of aircraft a flight is, as the Source reports it: a short ICAO **type
designator** (e.g. `B738`, `C172`) for compact display, and, when available, a
longer human **model** description (e.g. "BOEING 737-800") for detail. Purely
descriptive — like the callsign, it is shown but *never* affects which flight is
the Nearest or the Pacing flight. Either may be missing (many GA and uncatalogued
aircraft have neither), and the flight is still tracked without it.
_Avoid_: treating type as identity — the **hex** (ICAO address) is the stable
identity across Snapshots; many aircraft share a type.

**Vertical rate**:
How fast a flight is climbing or descending, in feet per minute (signed; a climb
is positive). Within a small floor of zero it reads as *level* — the list shows a
level glyph rather than a misleadingly precise trend. Display-only: like altitude,
it never affects the Nearest or Pacing flight.
_Note_: the Source may report a barometric and a geometric rate; the domain carries
one neutral number, preferring the barometric one.

**Barometric altitude** / **geometric altitude**:
Two altitudes a Source may report. *Barometric* altitude is pressure-derived (what
ATC means by "altitude"); *geometric* altitude is GNSS-derived height. Both are
display-only and either may be absent. The Nearest flight is measured by *ground*
distance only — neither altitude is part of it.
_Avoid_: treating the two as interchangeable; they can differ by hundreds of feet.

**Registration** (tail number):
The aircraft's civil registration (e.g. `N292WN`), tied to the airframe rather than
the flight. Descriptive only.
_Avoid_: using it as identity — the **hex** is the stable identity; a registration
can be absent and is reassigned across airframes over time.

**Operator**:
The owner/operating organisation a Source attributes to the aircraft (e.g.
"SOUTHWEST AIRLINES CO"). Descriptive only; often absent for GA aircraft.

**Squawk**:
The four-digit transponder code (an octal code, e.g. `1200`, `7700`) a flight is
broadcasting. Carried as text, never as a number. Descriptive only.
_Note_: emergencies surface through the separate *emergency* state, not by the app
interpreting special squawks itself.

**Emitter category**:
A Source's coarse classification of the aircraft (light, large, heavy, rotorcraft,
…), decoded by the adapter from its wire code. Descriptive only; absent when the
Source doesn't classify the aircraft.
_Avoid_: treating the raw code (`A3`) as domain language — the domain carries the
decoded human label; the code lives only inside the adapter.

**Flight details**:
Source-contributed, display-only telemetry shown *only* in the flight-detail
popup — signal strength, message counts, position-integrity figures,
navigation/autopilot selections, data provenance, and the like. The Source
pre-formats and groups these into labelled `(label, value)` pairs; the app renders
them verbatim and reasons about none of them. Like aircraft type and model, they
are purely descriptive — they never affect the Nearest or Pacing flight, and a
Source that supplies none simply yields an empty popup body.
_Avoid_: naming specific wire fields (RSSI, NIC, MLAT, …) anywhere above the
adapter — by design they exist only as opaque strings in a detail group (ADR-0004).

## Example dialogue

> **Dev:** If a jet is directly overhead at 36,000 ft, is that the nearest flight?
> **Domain expert:** Yes. We measure ground distance, so overhead is ~0 — it wins.
>
> **Dev:** What if a closer plane is just outside the Search area?
> **Domain expert:** Then we don't see it. "Nearest" only means nearest *among
> what's in the Search area*. Sizing the radius is how you trade coverage against
> request volume.
>
> **Dev:** Does Home ever move if I take the laptop somewhere?
> **Domain expert:** No. Home is fixed in config. That's the whole point of the
> word — if we wanted device location we'd call it something else.
>
> **Dev:** A jet just passed overhead and is heading away, but a second plane is
> inbound 200 km out. What's on screen, and what sets our next poll?
> **Domain expert:** Screen shows the jet — it's still the Nearest flight. But it's
> receding, so it doesn't pace us. The inbound plane is the Pacing flight; its CPA
> is soonest among relevant approaches, so it decides when we poll next.
>
> **Dev:** Between polls, how does the jet's distance keep updating if we're not
> calling the API?
> **Domain expert:** Dead reckoning — we move it along its last known heading and
> speed. The next poll just corrects however far that estimate has drifted.
>
> **Dev:** If I buy an ADS-B receiver later, does any of this change?
> **Domain expert:** No — it's just another Source behind the same interface. The
> poller, tracker, and radar don't notice; you point config at it, and it declares
> a faster minimum poll interval because there's no rate limit to respect.
