+++
title = "Auto Exposure"
description = "zenfilters.auto_exposure — filter node"
weight = 400

[taxonomies]
tags = ["auto", "exposure", "normalize"]

[extra]
node_id = "zenfilters.auto_exposure"
role = "filter"
group = "auto"
stage = "Filters"
+++

Automatic exposure correction by normalizing to a target middle grey.  Measures the geometric mean of L (log-average luminance) and applies exposure correction to bring it to the target. The geometric mean is robust against small bright areas that would bias an arithmetic mean.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_correction` | float (0.5 – 5.0) | 2.0 | Maximum correction in stops (prevents extreme adjustments) (EV) |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Correction strength (0 = off, 1 = full correction to target) |
| `target` | float (0.20000000298023224 – 0.800000011920929) | 0.5 | Target middle grey in Oklab L |

