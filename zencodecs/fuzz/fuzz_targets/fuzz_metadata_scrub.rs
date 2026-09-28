#![no_main]
use libfuzzer_sys::fuzz_target;
use zencodecs::metadata::ScrubRequest;
fuzz_target!(|data: &[u8]| {
    if let Ok(plan) = ScrubRequest::new(data)
        .with_byte_limits(1 << 20, 2 << 20)
        .with_metadata_limit(1 << 16)
        .with_carrier_limit(4096)
        .plan()
    {
        assert_eq!(plan.output_len(), plan.chunks().map(<[u8]>::len).sum::<usize>());
        for entry in plan.report() {
            assert!(entry.source_range().end <= data.len());
        }
        assert_eq!(plan.to_vec().unwrap().len(), plan.output_len());
    }
});
