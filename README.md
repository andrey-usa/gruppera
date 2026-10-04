# gruppera

High-performance station-temperature aggregation engine, written in Rust.

## Status

Early development. The engine currently aggregates 100M rows (`station;temperature`
lines) in ~0.9s on 2 cores — 2.3× faster than the winning 1BRC Java submission
on the same hardware, with byte-identical output.

## Design

- Memory-mapped input, 2MB work-stealing chunks claimed via an atomic cursor
- Three interleaved row parsers per thread to hide hash-table latency
- SWAR branchless temperature parsing into integer tenths
- Per-thread open-addressing hash tables (64-byte slots, linear probing)
- Integer-only accumulation; floating point only at output formatting

The techniques follow standard practice in high-performance text aggregation.
We are grateful to the 1BRC (One Billion Row Challenge) community, whose
public write-ups informed this design. This is an original implementation.

## Build

```sh
cd engine
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## Run

```sh
./target/release/gruppera measurements.txt
```

## License

TBD — the author will choose a license before the first release.
