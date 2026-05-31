# Retained, merged tracks with contact state — not wholesale-replace snapshots

ADR-0002 had the Tracker hold a single **Snapshot** and replace it wholesale on
every poll: a flight the new poll omitted was assumed to have left the area and
dropped on the spot. But ADS-B/MLAT feeds are jittery — aircraft routinely drop
out of one poll and return in the next — so wholesale replacement turned ordinary
feed jitter into flicker: blips vanishing and reappearing, and `/flight/{hex}`
404ing mid-popup. The old `max_flight_age` cap only protected the case where the
*whole feed* stopped (dead-reckoning the held snapshot); it did nothing for a
single flight missing from an otherwise-healthy poll.

We invert the model. The Tracker now holds a **retained set of tracks keyed by
hex**, and a Snapshot is **merged** into it rather than swapped in. A flight a poll
reports refreshes its track and is **in contact**; a flight a *successful* poll
omits is **lost** — kept, **frozen** at its last-known position, marked, and held
until it ages past the **staleness cap** (`3 × max_poll`), uniform across the
reasons. That cap sits one poll-interval *above* the whole-Picture stale threshold
(`2 × max_poll`), so a feed outage flags the Picture stale while its last-known
flights still glide for a beat before being dropped — not blanked the instant the
flag trips. A lost track carries *why*, as far as the data substantiates:
**landed** (the feed reported the hex on the ground — the only reason we confirm
rather than infer), **left scope** (its last heading would now carry it outside
the Search radius — geometry), or plain **lost contact** (the honest residual; we
don't guess). The per-flight contact state is computed server-side and carried on
the wire as a `state` field (ADR-0005: the Server decides, the Client renders);
`age_s` becomes time-since-contact and drives a client-side fade. Lost flights can
still be the **Nearest** flight (badged) but never **pace** — we won't spend API
calls chasing a frozen, stale CPA.

## Considered Options

- **Keep wholesale-replace, retain only the open detail flight** — rejected: fixes
  the popup but not the list/radar flicker, and leaves the core model lying about
  what "omitted" means.
- **Dead-reckon lost flights too (glide them on)** — rejected: dead reckoning is
  justified only because the next poll corrects its drift; for a lost flight no
  correction is coming, so gliding accumulates unbounded fiction — and a flight
  that quietly landed would sail across the map. Freezing is the honest choice, and
  in-scope flights are close and slow on screen, so a frozen position barely
  diverges from reality.
- **Infer "landed" from a low, descending last report** — rejected: a low flight
  that merely lost coverage looks identical; we'd mislabel dropouts as landings.
  We take the feed's real on-ground signal (high precision) and accept that a
  landing we never get a ground report for stays "lost contact" (lower recall).
- **Server hysteresis before marking lost** — rejected: `lost` means precisely
  "omitted by a poll that succeeded for others," a crisp threshold-free fact; the
  Client smooths one-poll blips by ramping emphasis with age instead.

## Consequences

- The domain **Snapshot** is redefined (CONTEXT.md): one poll's contribution to the
  held state, not the held state itself — reversing ADR-0002's "omitted ⇒ left the
  area." Whole-Picture `Stale`/`NoData` health (a feed outage) now coexists with
  per-flight lost-contact (one flight missing while others arrive); a total outage
  does **not** freeze flights — no successful poll is omitting them — so they keep
  dead-reckoning, and because the staleness cap (`3 × max_poll`) now sits above the
  stale threshold (`2 × max_poll`), they glide on under a STALE Picture for one
  poll-interval before the cap finally drops them (`tracker_cfg`). The old config
  tied the two thresholds, collapsing that window; splitting them makes the
  graceful-degradation behavior this document describes actually observable.
- The airborne-only invariant (ADR-0003) is *relaxed at the seam*: the readsb
  adapter still maps only airborne aircraft to `Flight`s, but a Snapshot now also
  carries `on_ground: Vec<hex>` so the Tracker can turn an *already-tracked* hex to
  "landed". Untracked ground hexes are ignored — no new ground blips.
- A new wire field (`Flight.state`) and the redefined `age_s` are additive but pin
  more of the contract clients depend on; the TUI's "flight left the area" notice
  now fires only after a lost track expires, with a stale-state banner until then,
  and the last-known detail (promoted fields + ADR-0004 groups) survives the whole
  lost window.
- Dead reckoning is now contact-gated, so `tracks_at` must reckon from each track's
  own last-seen instant rather than one snapshot timestamp.
