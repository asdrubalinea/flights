# The webclient becomes an Airbus glass-cockpit emulation, diverging from the TUI

The webclient (ADR-0007) is restyled from a generic dark radar into an Airbus-cockpit
emulation: an ND/TCAS **scope** beside an **MCDU**-style console, set in **B612 Mono**
(the font Airbus commissioned for cockpit screens) under the Airbus **EFIS colour
philosophy**. Colour stops being decorative and starts signalling — so the webclient's
role colours deliberately **diverge** from the TUI's `blip_color`, and the
selected-flight view becomes a purpose-built **instrument cluster that shows only
ADS-B-derived values, never fabricated ones**.

*Why diverge from the TUI's colours, when `display.rs` was kept in lockstep with it?*
EFIS colours carry meaning (red=warning, amber=caution, green=active, cyan=for-info,
magenta=managed-target, white=present). Mapping our roles onto them surfaces two states
the old palette buried — **emergency** (red, with an ECAM-style master-warning) and
**lost/stale** contact (amber, ADR-0007). The cost is web/TUI colour parity, which was
a nicety, not a contract — colour is Client display policy (CONTEXT.md). The TUI may
follow later, or not.

*Why no artificial horizon (or airspeed, or engine pages)?* ADS-B — all `flights-api`
carries — has no attitude, no airspeed (only **groundspeed**), no heading (only **ground
track**), and no systems data (see *Observed kinematics*, CONTEXT.md). Rather than fake
them (against the thin-client / "never fabricated values" spine of ADR-0005/0007) or
blank them behind failure flags, we **redesign** the panel so its gauge set *is* the data
we hold (ALT/VS/GS/TRK/FL/squawk/CPA), in full glass-cockpit livery. It is a ground-side
reconstruction of the flight, not the aircraft's own cockpit.

## Considered Options

- **Keep TUI-parity colours, restyle chrome only** — rejected: leaves the palette
  decorative and emergency/lost buried; wastes the EFIS philosophy.
- **Faithfully reproduce the A320 PFD, red-flagging the missing instruments** — rejected:
  a half-blanked panel foregrounds what we *lack*; designing around what we *have* looks
  complete and stays honest.
- **Full-screen instrument takeover for the selected flight** — rejected: it hides the
  live home-scope; we keep the scope always-visible and fit a compact cluster in the
  console DETAIL page instead.
- **Monochrome amber phosphor (Boeing-CDU look)** — rejected: the real Airbus MCDU is
  colourful; monochrome is a different aircraft maker's aesthetic.

## Consequences

`display.rs`'s colour constants fork from the TUI's; the in-comment promise of mirroring
the TUI is replaced by the EFIS mapping (web-internal canvas/DOM parity still holds). The
dev shell gains a bundled font asset (B612 Mono `woff2`) via Trunk `copy-file` — still no
npm (ADR-0007 holds). `Mode::Detail` changes from a floating modal to an in-console page,
and the **emergency** state — previously detail-only — gains a scope symbol and a
master-warning. That warning keys off the Server's `emergency` field, never the client
interpreting special squawk codes (CONTEXT.md, *Squawk*). CRT scanlines/curvature sit
behind `?crt=0`. The instrument set is bounded by what `flights-api` promotes (ADR-0004):
if the Server later promotes Mode-S EHS fields (roll, selected heading, IAS) more gauges
could light up — until then they are deliberately absent. The API stays unversioned; the
webclient updates in lockstep with `flights-api` (ADR-0005).
