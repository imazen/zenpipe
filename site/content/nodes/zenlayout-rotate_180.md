+++
title = "Rotate180"
description = "zenlayout.rotate_180 — orient node"
weight = 30

[taxonomies]
tags = ["rotate", "geometry"]

[extra]
node_id = "zenlayout.rotate_180"
role = "orient"
group = "geometry"
+++

Rotate the image 180 degrees.  Decomposes to flip-H + flip-V (no axis swap), so it can be coalesced into the layout plan without pixel materialization.  RIAPI: `?srotate=180`

