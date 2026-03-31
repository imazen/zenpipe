# ⚙️ Tone Curve

> **ID:** `zenfilters.tone_curve` · **Role:** filter · **Group:** tone
> **Tags:** `tone`, `curve`

Arbitrary tone curve via control points with cubic spline interpolation  Control points define an input→output mapping on the L channel. Points are encoded as a comma-separated string of "x:y" pairs, e.g., "0.0:0.0,0.25:0.15,0.75:0.85,1.0:1.0". The execution layer parses this and calls ToneCurve::from_points().

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `points` | string | 0:0,1:1 | Control points as "x:y" pairs, comma-separated.  Each point is input_L:output_L in [0,1] range. Default is identity (diagonal line): "0:0,1:1". Example S-curve: "0:0,0.25:0.15,0.75:0.85,1:1". |
