# Long-running process with adaptive CPA-paced polling, not stateless per-tick calls

Calling a flight Source on every screen refresh is wasteful and, depending on the
Source, expensive or rate-limited: a metered API (AeroAPI) bills per query, and a
free community API (airplanes.live) caps requests at ≈1/s. `flights` instead runs
as a single long-lived process that holds flight state in memory and decides its
own poll cadence: it polls fast only when an *approaching, relevant* flight's
closest point of approach (CPA) is imminent, and backs off when the airspace is
quiet, so request volume tracks how interesting the airspace is. Between polls the
display is kept current by **dead reckoning** (extrapolating last-known position
along last-known velocity), so screen smoothness costs nothing. Poll cadence is
bounded below by the active Source's declared minimum interval (see
[ADR-0003](0003-pluggable-data-sources.md)) — near-zero for a local receiver, ≈1 s
for airplanes.live, a cost ceiling for a paid API — and above by Search-area
transit time (so a fast jet can't cross unseen).

## Considered Options

- **Stateless CLI re-run by waybar each tick** — rejected: every tick is a fresh
  request (a billed query or a rate-limited one), with no way to anticipate an
  approach or dead-reckon between calls; request volume scales directly with the
  bar's refresh rate.
- **Fixed-interval polling** — rejected: a single interval either over-pays when
  the sky is quiet or under-samples a fast approach.

## Consequences

Requires a long-running runtime and retained per-flight state — a poller plus a
UI, rather than a one-shot script. The eventual waybar integration becomes "read
this process's output," not "spawn this script." CPA is a straight-line estimate
refreshed on every poll, so maneuvering aircraft self-correct on the next call.
