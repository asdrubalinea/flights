# Hybrid flight data model: typed cross-provider fields plus opaque grouped details

The flight-detail popup must surface every field the upstream feed provides (no
extra API calls), which for the readsb/airplanes.live family spans ~40 wire keys.
ADR-0003 makes the domain `Flight` the stable, **provider-neutral** contract — it
must not grow to mirror one provider's wire schema. Yet some of those fields are
cross-provider and used by core UI/logic (the list's vertical-trend glyph, the
popup header), while most are long-tail, provider-specific, and shown only as
read-only text (RSSI, message counts, NIC/NACp/SIL integrity, MLAT/TIS-B
provenance, ADS-B version, autopilot `nav_*` selections, QNH, …). How we hold
"all upstream fields" decides whether the neutral domain stays clean or absorbs
one provider's shape.

We split the fields by role:

1. **Promote** the small, cross-provider, logic-bearing set to typed `Option`
   fields on `Flight` (vertical rate, geometric altitude, registration, operator,
   squawk, emergency, emitter category). The adapter maps wire → typed; the UI
   formats them. This is the existing pattern for altitude/type/model.
2. Carry the long tail as a generic `details: Vec<DetailGroup>` of **already
   formatted, grouped** `(label, value)` strings. The adapter — the only layer that
   understands these wire fields — decodes, labels, and groups them ("Signal",
   "Integrity", "Navigation", "Provenance"). The popup renders them verbatim with
   no per-Source code. The domain names none of them.

## Considered Options

- **Flatten everything into typed `Flight` fields** — rejected: bloats the
  provider-neutral contract with ~40 fields, most provider-specific and meaningless
  to other Sources (OpenSky, a paid API), violating ADR-0003; every new Source
  would stub fields it cannot supply, and the domain would encode one vendor's
  schema.
- **Carry a raw provider map** (`HashMap<String, Value>` / raw JSON) — rejected:
  leaks wire keys and provider encodings into the domain and forces the UI to know
  provider specifics to format them — again violating ADR-0003, and dropping type
  safety for the promoted fields.
- **Hybrid: typed promotions + opaque grouped details (chosen)** — the core stays
  small and typed; the long tail is provider-formatted text the UI shows blindly.
  A new Source promotes what it shares and fills detail groups with whatever else
  it has, with zero UI change.

## Consequences

The adapter owns all wire-code decoding (emitter category, data-source type) and
detail grouping/formatting, keeping ADR-0003's "capability differences absorbed
inside adapters" intact. The popup becomes a generic renderer: surfacing a new
wire field is an adapter-only change. Detail values are display-only strings,
never parsed back, and — like type and model — never influence the Nearest or
Pacing flight. The cost is a soft boundary: deciding whether a new field is
"promote-worthy" (cross-provider and used by logic) or "just a detail" is a
judgement call made per field.
