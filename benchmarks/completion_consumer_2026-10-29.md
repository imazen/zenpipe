# Owned completion consumer qualification

Source: ff5d59a29e03b568c11e56ba1883b10b6b663243, parent 5d93e34a.
Core eeb1a4504c9ed83dd27a9459107260d0a78a47fb; JPEG
c73a3cc90997c0553832243daba383ac3086dc98; pixels f23ac753 unchanged.
Ran in an isolated export outside sibling path overrides.

Commands, sequential under run-heavy --mem 3G --jobs 2 and timeout 900:

```sh
just checked-publication-locks
cargo check --locked --all-targets --manifest-path fuzz/Cargo.toml
cargo check --locked --all-targets --manifest-path zencodecs/fuzz/Cargo.toml
just checked-publication-check
cargo test -p zenpipe-io-check --features job --locked
cargo clippy -p zenpipe-io-check --features job --all-targets --locked -- -D warnings
```

All commands passed. Wrapper: rc=0, 46s, peak-RSS 0.66GiB,
min-avail 11067MiB, peak-load 0.77. These are qualification resource observations,
not throughput comparisons or native workspace bounds.
Lock package-record symmetric differences contain only the core family plus
zenjpeg/zenyuv across all five roots. Test summaries:

```text
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 160 filtered out; finished in 0.01s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.67s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.03s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

AVIF orientation assertions remain unchanged and unresolved; this focused run
does not qualify that route or the complete application feature matrix.
