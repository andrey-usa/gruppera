# gruppera

High-performance station-temperature aggregation engine, written in Rust.

## Status

On 200M rows (2.6 GB, official 1BRC generator, warm cache,
4 vCPU GitHub runner) gruppera is within 5% of the 1BRC winner (thomaswue)
built the way it ships, as a GraalVM native image, ties it on CPU time, and
runs 1.76× faster than the winner on the JVM — with byte-identical output
([bench-200m run 37402714819](https://github.com/andrey-usa/gruppera/actions/runs/37402714819)):

| contender | wall (best of 3) | CPU time |
|---|---|---|
| gruppera | 0.915 s | 3.35 s |
| gruppera + PGO | 0.913 s | 3.34 s |
| winner, native image (tuned flags) | 0.875 s | 3.36 s |
| winner, native image (default) | 0.885 s | 3.37 s |
| winner, OpenJDK 21 (JIT) | 1.609 s | 5.59 s |

The native image's remaining wall-time edge includes its fork trick: the parent prints and exits while a worker process
still unmaps the file, so the measured wall stops before teardown; gruppera's
wall includes its own munmap.

## Design

- Memory-mapped input, 2MB work-stealing chunks claimed via an atomic cursor
- Three interleaved row parsers per thread to hide hash-table latency
- SWAR branchless temperature parsing into integer tenths
- Per-thread open-addressing hash tables (64-byte slots, linear probing)
- Integer-only accumulation; floating point only at output formatting

The techniques follow standard practice in high-performance text aggregation.
We are grateful to the 1BRC (One Billion Row Challenge) community, whose
public write-ups informed this design. This is an original implementation.

## Correctness

Every push runs `tests/fuzz.py`: the release binary (real mmap) is diffed
against a naive reference on randomized inputs — name lengths around every
SWAR path boundary, multibyte UTF-8, 100-byte names, 10k-station multi-chunk
files, tiny files — with half the inputs ending exactly on a 4 KiB page so
any read past EOF faults. Output order matches Java's `TreeMap<String>`
(UTF-16 code units).

## Install

Prebuilt binaries for Linux (x86_64, aarch64), macOS (arm64, x86_64) and
Windows (x64) are on the [releases page](https://github.com/andrey-usa/gruppera/releases),
and the same binaries through package managers:

| | |
|---|---|
| Cargo | `cargo install --locked gruppera` |
| npm | `npm install -g gruppera` |
| pip / uv | `pip install gruppera` · `uv tool install gruppera` |
| Homebrew | `brew install andrey-usa/tap/gruppera` |
| Scoop | `scoop bucket add andrey-usa https://github.com/andrey-usa/scoop-bucket` then `scoop install gruppera` |

The prebuilt x86_64 binaries target `x86-64-v2` so they run on any CPU from
the last fifteen years. For the numbers above, build for your own machine:

```sh
cargo build --release      # .cargo/config.toml sets -C target-cpu=native
```

## Run

```sh
gruppera measurements.txt
```

Input is the 1BRC format, one `<station>;<temperature>` row per line;
output is `{station=min/mean/max, ...}` sorted by station name.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
