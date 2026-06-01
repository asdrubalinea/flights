# Flights

A long-running **Server** that tracks the **nearest flight** to a fixed **Home**
location using a pluggable **Source** (initially the free, community-run
airplanes.live ADS-B API), deciding *itself* how often to poll so it stays within
each Source's limits. It crunches the data once and exposes the result over a
small, cheap local REST API consumed by thin **Clients**: a radar-style TUI (a
Home-centered scope of gliding blips beside a list of those flights), and a
webclient later. Each Client only renders what the Server computes — none of them
touch a Source.

## Language

**Server**:
The single long-running process that owns the Source, polls it on its self-chosen
cadence, holds the latest **Snapshot**, and answers every question about the
airspace — **dead-reckoning** each flight to the *instant of the request*. It is
the only thing that talks to a Source and the sole source of truth.
_Avoid_: "backend", "daemon" as domain terms (just "the Server").

**Client**:
Any consumer of the Server's API that only *renders* — the TUI, the webclient, and
the **waybar module**. A Client chooses *what* to show and *how often* to ask, but
never computes which flight is nearest or where a blip belongs; that is always the
Server's answer. Whether a flight is close enough to be worth showing is Client
policy: the radar shows *every* flight in the Search area, while the single-line
waybar module shows only the **Nearest flight**, and only while it sits within that
module's **Display range** (else the bar stays empty). This display policy is
distinct from **Relevance distance** (which gates the Server's pacing, never a
Client's display).
_Note_: projecting the Server's answer onto a particular screen — polar
(`distance_nm`, `bearing_deg`) → pixels on a canvas of a given size, scaled to the
viewport — is display, not geometry. A Client may scale and place blips for its own
viewport; what it may never do is *derive* a flight's position, distance, bearing, or
which flight is nearest. Drawing a Server answer is not computing one.

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

**Display range**:
The ground-distance cutoff a Client uses to decide whether the **Nearest flight**
is close enough to be worth putting on screen at all. Purely Client display policy:
it never reaches the Server, and is deliberately a *third*, distinct distance from
both the **Search area** radius (the Server's coverage) and the **Relevance
distance** (the Server's pacing gate). A Client may show nothing while the Nearest
flight lies beyond its Display range, even though the Server still tracks it and
still answers `/nearest` with it. Where the radar shows every flight in the Search
area, a single-line bar Client uses a Display range to stay quiet until something is
genuinely overhead.
_Avoid_: "Relevance distance" (that gates *Server* pacing), "radius" / "Search
area" (Server coverage). Different number, different owner, different purpose.

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
_Only flights **in contact** are dead-reckoned._ A flight we have **lost
contact** with is *frozen* at its last-known position, never extrapolated: there
is no poll coming to correct it, so gliding it onward would be fiction (a plane
that has quietly landed must not keep sliding across the scope). Within the Search
area flights are close and slow on screen, so a frozen last-known position barely
diverges from reality — stale is cheap; fiction is not.

**Snapshot**:
The set of airborne flights in the Search area returned by a *single* poll,
authoritative as of its timestamp. A Snapshot is **merged** into the retained set
of tracked flights — it is not swapped in wholesale. A flight the poll reports
refreshes its track and is **in contact**; a flight the poll *omits* is **not**
assumed gone — it has merely not been heard this poll, so its track is kept and
becomes **lost** (see below). This deliberately reverses the older
"omitted ⇒ left the area" rule, which turned ordinary feed jitter — flights
dropping in and out of consecutive polls — into flicker.
_Avoid_: treating a Snapshot as the whole held state; it is one poll's
contribution to it. The complete held view at an instant is the **Picture**.

**Contact** / **In contact** / **Lost contact**:
A track's **contact state**. A flight is **in contact** when the latest poll
reported it. When a poll that *succeeded* (returned others) omits a flight we
already track, we have **lost contact**: the track is retained, *frozen* at its
last-known position (never dead-reckoned — see **Dead reckoning**), visibly marked,
and kept until it ages past the **staleness cap**, after which it is finally
dropped. The cap is uniform across the reasons below. A flight is marked lost the
moment a successful poll omits it (no tolerance delay); a Client softens that
into a graceful fade by leaning on each track's *age*, so a one-poll coverage blip
doesn't flash an alarm.

Losing contact is a *per-flight* fact, distinct from the whole-**Picture** going
*stale*. A single flight can be lost while polls keep arriving for everything else.
And the converse: a *total* poll outage (every poll failing) does **not** mark
flights lost — no successful poll is omitting them — so they keep being
dead-reckoned, gliding on under a stale Picture until the staleness cap clears
them. (The cap sits a poll-interval above the stale flag, so an outage shows
last-known motion rather than blanking the moment the Picture goes stale.) Only a
successful-poll omission freezes a flight.

A lost track carries *why* contact was lost, as far as the data can substantiate:
- **Landed** — the Source reported this aircraft on the ground. The one
  disappearance reason we can confirm from real data rather than infer. On-ground
  aircraft are otherwise not flights at all (the feed is airborne-only); only a hex
  we were *already* tracking can turn "landed".
- **Left the Search area** ("left scope") — had the flight held its last course,
  it would now lie outside the Search **radius**: a near-edge last position with an
  outbound heading. This hypothetical extrapolation decides the *reason* only; the
  displayed blip still stays **frozen** at the last-known position, never glided
  out past the edge. Pure geometry; high confidence.
- **Lost contact** (plain) — the honest residual: omitted, not on the ground, not
  outbound. Cause genuinely unknown (a coverage gap, an MLAT dropout, a landing we
  never got the ground report for); we do not guess at one.
_Avoid_: inferring "landed" from a low, descending last report — a low flight that
merely dropped out looks identical, and guessing it would show fiction.

**Track**:
A single flight as estimated at a particular instant — its position plus the
geometry derived from it: ground distance and bearing from Home, and its **CPA**.
A Track persists across polls and carries a **contact state** (**in contact**,
or **lost** with a reason — see **Contact**). An in-contact Track is
**dead-reckoned** to the query instant; a lost Track is *frozen* at its last-known
position. A Track is what a Client renders; it is computed *from* the raw reported
flight a **Snapshot** last carried, never the same thing.
_Note_: "track" is overloaded, as it is in real ATC usage — a *Track* (this term,
a tracked target) versus *track* the ground heading in degrees (a flight's
direction of travel). Both senses are kept; context disambiguates.

**Picture**:
The complete, self-consistent view of the airspace at one instant: every **Track**,
which one is the **Nearest flight**, which is the **Pacing flight**, and how fresh
the data is. Derived from a *single* dead-reckoning pass, so the radar, list, and
status can never disagree — they read one Picture rather than recomputing three
times. The Server's primary API answer.

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
>
> **Dev:** When a Client asks for the nearest flight, does that trigger a
> Source poll?
> **Domain expert:** No. The Server polls on its own cadence and holds the latest
> Snapshot; a Client request just dead-reckons that Snapshot to *now* and answers.
> Clients are cheap to serve precisely because asking the Server costs no API call —
> the only thing that ever touches a Source is the Server's own poller.
>
> **Dev:** So if two Clients are open at once, are we polling
> airplanes.live twice?
> **Domain expert:** No — there's one Server and one poller. Every Client reads the
> same Picture. That's the whole reason the engine lives only in the Server: two
> processes polling a 1-req/s Source would blow the budget.
>
> **Dev:** A flight is on the scope, then the next poll just… doesn't include it.
> Gone?
> **Domain expert:** No — that's exactly what we refuse to do now. A poll omitting
> a flight means we've *lost contact*, not that it vanished. We keep the track,
> freeze it where we last saw it, mark it lost, and hold it until it ages past the
> staleness cap. ADS-B jitter drops aircraft in and out constantly; losing the blip
> every time would be flicker.
>
> **Dev:** Frozen — so we stop sliding it along its heading?
> **Domain expert:** Right. We dead-reckon a flight only while we're hearing it,
> because the next poll corrects the drift. For a lost flight no correction is
> coming, so gliding it on would be fiction — and if it had quietly landed we'd be
> sailing a parked plane across the map. It sits at its last-known spot, stale.
>
> **Dev:** How do we ever know it actually landed versus just dropped out?
> **Domain expert:** Only if the feed tells us the aircraft is on the ground — that
> we trust. Otherwise we don't guess: a low flight that merely lost coverage looks
> identical, so it stays plain "lost contact." The one inference we *do* make is
> "left the Search area" — if its last heading would carry it outside the radius,
> that's geometry, not a guess.
>
> **Dev:** If I had its detail popup open when it dropped out, do I lose everything?
> **Domain expert:** No. The last-known detail stays — every field, every group —
> marked stale, for as long as the track lives. You only get the "gone" notice once
> it ages out for good.
>
> **Dev:** A jet we lost was the nearest, and it was inbound. Does it still set our
> poll rate?
> **Domain expert:** It can still be shown as the Nearest — badged lost — because
> it's still the closest thing we know of. But it never *paces*: we won't burn API
> calls chasing a frozen, stale CPA. Only flights we're actually hearing pace us.
