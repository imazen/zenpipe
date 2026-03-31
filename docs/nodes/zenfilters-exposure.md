# ⚙️ Exposure

> **ID:** `zenfilters.exposure` · **Role:** filter · **Group:** tone

Exposure adjustment in photographic stops.  +1 stop doubles linear light, -1 halves it. Preserves hue and saturation by scaling all Oklab channels proportionally.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `stops` | float (-5.0 – 5.0) | 0.0 | Exposure compensation in stops (+/-)  Note: RIAPI `s.brightness` historically used a -1..1 sRGB offset, not photographic stops. The kv alias is provided for discoverability; callers should be aware of the different scale. (EV) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.brightness` | — | `stops` |

**Example:** `?s.brightness=value`
