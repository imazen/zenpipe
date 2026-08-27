//! Execution-layer tracing: per-strip events, execution finalization, and
//! phase timing (zenpipe#8). Needs `std` (tracing is std-only).

#![cfg(feature = "std")]

use hashbrown::HashMap;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::CallbackSource;
use zenpipe::trace::{ExecutionPhase, FullPipelineTrace, PipelineTrace, TraceConfig};
use zenpipe::{Source, format};

fn solid_source(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].as_chunks_mut::<4>().0.iter_mut() {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

fn drain(source: &mut dyn Source) -> u32 {
    let mut strips = 0;
    while let Some(_strip) = source.next().unwrap() {
        strips += 1;
    }
    strips
}

fn traced(config: &TraceConfig) -> (u32, PipelineTrace) {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 64,
        h: 64,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);
    let mut sources = HashMap::new();
    sources.insert(src, solid_source(128, 256, [10, 20, 30, 255]));
    let (mut pipeline, trace) = g.compile_traced(sources, config).unwrap();
    let strips = drain(pipeline.as_mut());
    let graph = trace.lock().unwrap().clone();
    (strips, graph)
}

#[test]
fn strip_events_record_every_pull_when_enabled() {
    let (pulled, graph) = traced(&TraceConfig::metadata_only().with_strip_events());
    assert!(
        pulled > 1,
        "fixture must produce several strips, got {pulled}"
    );
    let out = graph.entries.last().expect("output entry");
    let t = out.timing.as_ref().expect("timing enabled").lock().unwrap();
    assert_eq!(t.strip_count, pulled);
    assert_eq!(
        t.strips.len() as u32,
        pulled,
        "one StripEvent per pull at the output node"
    );
    // Events index sequentially and account for every byte NodeTiming saw.
    for (i, ev) in t.strips.iter().enumerate() {
        assert_eq!(ev.strip_num, i as u32);
        assert!(ev.rows > 0);
        assert_eq!(ev.bytes, 64 * 4 * ev.rows as u64);
    }
    assert_eq!(
        t.strips.iter().map(|e| e.bytes).sum::<u64>(),
        t.bytes_processed
    );
    // total_duration also counts the final EOF pull (no strip), so the events
    // account for at most the total.
    let events_total = t
        .strips
        .iter()
        .map(|e| e.duration)
        .sum::<std::time::Duration>();
    assert!(events_total <= t.total_duration);
    assert!(events_total > std::time::Duration::ZERO);
}

#[test]
fn strip_events_off_by_default_even_with_timing() {
    let mut cfg = TraceConfig::metadata_only();
    cfg.timing = true;
    let (pulled, graph) = traced(&cfg);
    let out = graph.entries.last().unwrap();
    let t = out.timing.as_ref().unwrap().lock().unwrap();
    assert_eq!(t.strip_count, pulled);
    assert!(t.strips.is_empty(), "no events unless strip_events is set");
}

#[test]
fn finish_execution_populates_totals_phases_and_slowest_strip() {
    let (pulled, graph) = traced(&TraceConfig::full());
    assert!(
        graph.compile_duration.is_some(),
        "compile_traced records the compilation phase"
    );
    let mut full = FullPipelineTrace {
        graph,
        ..Default::default()
    };
    assert!(full.execution.is_none());
    let exec = full.finish_execution().clone();
    assert_eq!(exec.total_strips, pulled);
    let out_total = full
        .graph
        .entries
        .last()
        .unwrap()
        .timing
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .total_duration;
    assert_eq!(exec.total_duration, out_total);
    assert!(
        exec.phases
            .iter()
            .any(|(p, _)| *p == ExecutionPhase::Compilation)
    );
    assert!(
        exec.phases
            .iter()
            .any(|(p, _)| *p == ExecutionPhase::Execution)
    );
    let slowest = exec.slowest_strip.expect("strip events were on");
    assert!(slowest.trace_order < full.graph.entries.len());

    let text = full.to_text();
    assert!(text.contains("Execution Trace"), "{text}");
    assert!(text.contains("phase Compilation"), "{text}");
    assert!(text.contains("slowest strip"), "{text}");
    let chart = full.strip_timing();
    assert!(chart.contains("Output") || chart.contains('#'), "{chart}");

    let json = full.to_json();
    assert!(json.contains("\"strip_events\":["), "{json}");
    assert!(json.contains("\"execution\":{"), "{json}");
    assert!(json.contains("\"phase\":\"Execution\""), "{json}");
}
