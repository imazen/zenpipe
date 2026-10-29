# Optional pipeline controls

```rust,ignore
// Ordinary use retains any source/sink controls; no extra per-call control.
zenpipe::execute(&mut source, &mut sink)?;

// A caller that already owns a Pulse passes it unchanged.
zenpipe::execute_with_pulse(&mut source, &mut sink, Some(pulse))?;

// A cancellation-only caller can borrow its token without an allocation.
let pulse = zencodec::how_far::StopOnly::borrowed(&deadline);
zenpipe::execute_with_pulse(&mut source, &mut sink, Some(&pulse))?;
```

`execute_with_pulse` checks before input pulls, after each pull and before sink
finish. Cancellation cannot roll back strips already consumed. The driver does
not create progress phases. `None` does not turn off source/sink-retained controls.
`execute_with_stop` retains its original required Stop signature and is deprecated.

```rust,ignore
let cached = session.stream_with_pulse(source, &config, sidecar, source_hash, Some(pulse))?;
// The returned source is lazy: this borrowed Pulse is not retained by it.
zenpipe::execute_with_pulse(&mut *cached.source, &mut sink, Some(pulse))?;
```

`Session::stream_with_pulse` checks before cache changes and forwards the same
optional Pulse into prefix materialization. `MaterializedSource::from_source_with_pulse`
checks before raster allocation and around source pulls. Their required-Stop
predecessors are deprecated compatibility adapters.

Source migration is in progress. Full consumer-graph, native-codec, API and browser
qualification is not implied by these examples. Animation now forwards the original optional Pulse through frame decode, frame
encode, finalization and strip execution. `FrameSource`/`FrameSink` expose
`*_with_pulse` variants; convenience methods use `None`. The old required-Stop
animation methods remain deprecated adapters. Lossless JPEG and zenfilters still
need the same public-boundary audit. Existing `PipeError::Cancelled`
still loses the native stop reason; changing that error contract remains open.
