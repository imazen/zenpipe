# Analysis Outputs

Pipeline nodes can produce structured data alongside pixel strips. Face bounding boxes, saliency regions, whitespace detection results — anything a node discovers during processing gets stored in a typed bag (`AnalysisOutputs`) and returned with the pipeline result.

## How it works

`AnalysisOutputs` is a `HashMap<&'static str, Box<dyn Any + Send>>`. Nodes write to it during pipeline compilation (when they materialize and analyze pixels). The caller retrieves typed results after the pipeline completes.

Two kinds of nodes write outputs:

1. **Built-in nodes** like `CropWhitespace` write automatically.
2. **Custom `Analyze` nodes** receive `&mut AnalysisOutputs` in their closure and write whatever they want.

The outputs bag lives on the pipeline result — `ProcessedImage::outputs` or `StreamingOutput::outputs`.

## Reading outputs

```rust
let result = zenpipe::process(source, &config)?;

// Built-in: CropWhitespace writes its detection result
if let Some(crop) = result.outputs.get::<CropWhitespaceResult>("crop_whitespace") {
    println!("content at ({}, {}), {}x{}, trimmed: {}",
        crop.content_x, crop.content_y,
        crop.content_width, crop.content_height,
        crop.trimmed);
}

// Custom: zenfaces writes face detections (hypothetical)
if let Some(faces) = result.outputs.get::<Vec<FaceDetection>>("zenfaces") {
    for face in faces {
        println!("face at ({}, {}) {}x{}, confidence: {:.2}",
            face.x, face.y, face.w, face.h, face.confidence);
    }
}
```

`get::<T>(key)` returns `None` if the key is missing or the type doesn't match. No panics.

## Writing outputs from an Analyze node

`NodeOp::Analyze` takes a closure that receives the materialized image and the outputs bag:

```rust
let analyze = graph.add_node(NodeOp::Analyze(Box::new(|mat, outputs| {
    // Run inference on the full image
    let faces = detect_faces(&mat);
    let saliency = compute_saliency_map(&mat);

    outputs.insert("faces", faces);
    outputs.insert("saliency", saliency);

    // Return pixels — unchanged, cropped, or transformed
    Ok(Box::new(mat))
})));
```

The closure can also modify the pixel stream based on analysis results. For example, a face-aware crop node might detect faces, write the detections to outputs, then return a cropped source centered on the primary face.

## Using compile_with_outputs directly

If you're building a `PipelineGraph` manually (without the orchestration layer), use `compile_with_outputs` to capture the bag:

```rust
let mut outputs = AnalysisOutputs::new();
let pipeline = graph.compile_with_outputs(sources, &mut outputs)?;

// Execute the pipeline...
zenpipe::execute(pipeline.as_mut(), &mut sink)?;

// Now read outputs
let crop = outputs.get::<CropWhitespaceResult>("crop_whitespace");
```

The convenience method `compile()` still works — it creates a throwaway bag internally. Use it when you don't care about analysis results.

## Built-in output keys

| Key | Type | Written by | Contents |
|-----|------|-----------|----------|
| `"crop_whitespace"` | `CropWhitespaceResult` | `NodeOp::CropWhitespace` | Source dims, content bounds (x, y, w, h), whether trimming occurred |

## Serialization

`AnalysisOutputs` stores `Box<dyn Any>`, so it can't be serialized directly. The intended pattern:

1. The node crate (e.g., zenfaces) defines its output type with `#[derive(Serialize)]`.
2. The caller knows which keys to expect and extracts typed values.
3. The caller serializes those values however it wants.

`CropWhitespaceResult` has `#[derive(serde::Serialize, serde::Deserialize)]` behind the `serde` feature gate.

With the `serde` feature enabled, `JobResultInfo` includes an `analysis_keys` field listing all output keys present in the bag. This tells API consumers which keys to look for without requiring the values themselves to be serializable through a common trait.

```rust
let info = JobResultInfo::from(&result);
// info.analysis_keys == ["crop_whitespace", "zenfaces"]
```

## Conventions for key names

Use the crate name as the key: `"zenfaces"`, `"zensally"`, `"crop_whitespace"`. This avoids collisions. If a crate writes multiple outputs, use dot-separated names: `"zenfaces.detections"`, `"zenfaces.landmarks"`.
