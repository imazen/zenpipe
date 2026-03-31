+++
title = "Rotate90"
description = "zenlayout.rotate_90 — orient node"
weight = 200

[taxonomies]
tags = ["rotate", "geometry"]

[extra]
node_id = "zenlayout.rotate_90"
role = "orient"
group = "geometry"
stage = "Orient & Crop"
+++

Rotate the image 90 degrees clockwise.  Swaps width and height. Coalesced with other geometry nodes so the layout planner computes correct dimensions through the full chain. Pixel axis-swap happens at execution time.  RIAPI: `?srotate=90`

