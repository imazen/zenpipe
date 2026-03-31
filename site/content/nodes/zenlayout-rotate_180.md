+++
title = "Rotate180"
description = "zenlayout.rotate_180 — orient node"
weight = 200

[taxonomies]
tags = ["rotate", "geometry"]

[extra]
node_id = "zenlayout.rotate_180"
role = "orient"
group = "geometry"
stage = "Orient & Crop"
+++

Rotate the image 180 degrees.  Decomposes to flip-H + flip-V (no axis swap), so it can be coalesced into the layout plan without pixel materialization.  RIAPI: `?srotate=180`

