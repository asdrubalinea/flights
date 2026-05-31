#!/usr/bin/env bash
# waybar module — the nearest flight to Home, as a one-liner over the Server's
# /nearest endpoint. The Server does all the work (ADR-0005); this only renders,
# and applies a *client-side* distance threshold: "show only when something is
# close." That threshold is Client display policy, distinct from the Server's
# Relevance distance (which gates pacing, never display) — see CONTEXT.md.
#
# Emits one line of waybar JSON: {"text","tooltip","class"}. Configure in waybar:
#
#   "custom/flights": {
#     "exec": "~/.config/waybar/flights-nearest.sh",
#     "return-type": "json",
#     "interval": 5
#   }
#
# Env overrides: FLIGHTS_URL (default http://127.0.0.1:7878),
#                FLIGHTS_MAX_NM (hide when the nearest is farther than this; default 30).

set -euo pipefail

URL="${FLIGHTS_URL:-http://127.0.0.1:7878}"
MAX_NM="${FLIGHTS_MAX_NM:-30}"

# A reachable Server is the only dependency besides jq. If curl fails, say so
# rather than emitting nothing (waybar would otherwise show a stale value).
if ! body="$(curl -fsS --max-time 4 "$URL/nearest" 2>/dev/null)"; then
  printf '%s\n' '{"text":"✈ —","tooltip":"flights-server unreachable","class":"down"}'
  exit 0
fi

# Render with jq. The Server already promoted every field and signed the CPA, so
# this is pure formatting:
#   - no flight in the area, or nearer than the threshold → blank module
#   - otherwise: callsign, distance, bearing; tooltip carries the detail.
printf '%s\n' "$body" | jq -c --argjson max "$MAX_NM" '
  def deg2compass:
    ["N","NE","E","SE","S","SW","W","NW"][((. / 45) + 0.5 | floor) % 8];
  if (.flight == null) or (.flight.distance_nm > $max) then
    {text:"", tooltip:"no flight within \($max) nm", class:"idle"}
  else
    .flight as $f
    | ($f.ident // "[\($f.hex)]") as $id
    | {
        text: "✈ \($id) \($f.distance_nm | floor)nm",
        tooltip: ([
            "\($id)\(if $f.type then " · \($f.type)" else "" end)",
            "\($f.distance_nm | floor) nm \($f.bearing_deg | deg2compass) · \($f.bearing_deg | floor)°",
            (if $f.altitude_ft then "\($f.altitude_ft | floor) ft \($f.vertical_trend)" else "altitude unknown" end),
            (if $f.cpa and ($f.cpa.time_to_cpa_s >= 0)
             then "CPA \($f.cpa.cpa_distance_nm | floor) nm in \($f.cpa.time_to_cpa_s | floor)s"
             else "receding" end)
          ] | join("\n")),
        class: (if $f.cpa and ($f.cpa.time_to_cpa_s >= 0) then "approaching" else "receding" end)
      }
  end
'
