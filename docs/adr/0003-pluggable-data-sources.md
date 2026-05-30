# Pluggable flight data sources behind a single trait

The app must be able to swap or add flight data providers — free community ADS-B
APIs now, a local ADS-B receiver or a paid API later — without touching the
poller, tracker, or UI. We define a `FlightSource` trait: given a Search area
(Home + radius) it returns a domain `Snapshot`. Each provider is an adapter that
maps its own wire format into the shared domain types (`Flight`, `Snapshot`),
which are the **stable internal contract**. The active provider is chosen at
runtime from config, and providers can be compiled in selectively via cargo
features so the default build stays lean.

The first adapter targets the readsb/tar1090-style JSON shared by
**airplanes.live**, **adsb.lol**, and **adsb.fi** (same schema, differing only by
base URL) — which means a future **local dump1090/readsb** instance is the *same*
adapter pointed at localhost, not new code. OpenSky (OAuth + bounding box) and any
paid API become their own adapters.

## Considered Options

- **Hard-code one provider** — rejected: changing or adding a source would mean
  rewriting the polling/UI layers, and the user explicitly wants to add sources
  (a receiver, others) later.
- **Trait + per-source adapters (chosen)** — sources are isolated behind one
  interface; the cost is the discipline of keeping domain types provider-neutral.

## Consequences

Domain types must not leak provider specifics. Capability differences are absorbed
inside adapters: a source that only accepts a bounding box (OpenSky) converts the
radius to a box and filters back; sources declare their own **minimum poll
interval** (near-zero for a local receiver, ≈1 s for airplanes.live, a cost
ceiling for a paid API), which becomes the lower bound the adaptive poller
(ADR-0002) respects — so plugging in a receiver automatically allows faster
polling with no change to the poller.
