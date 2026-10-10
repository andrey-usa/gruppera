# Contributing to gruppera

gruppera is built on `unsafe` SWAR code, and speed only counts when the output
is byte-identical to the Java reference for every valid input. The fuzz suite
has caught bugs no benchmark dataset exercised (a segfault on a 4096-byte file,
a sort-order mismatch), so correctness comes first in every change.

## Build and test

```sh
cargo build --release
python3 tests/fuzz.py target/release/gruppera 120   # differential fuzz vs a naive reference
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`tests/fuzz.py` diffs the release binary (real mmap) against a reference
implementation on randomized inputs. Even seeds end exactly on a 4 KiB page
boundary, so any read past EOF faults instead of hitting zero padding.

## Rules for `unsafe` changes

- Every load must provably stay inside the mapping, including the scalar tail
  path (`TAIL_MARGIN` covers only the vectorized workers).
- Any new parse, hash or compare path gets fuzz coverage at its boundaries:
  8/15/16/17-byte names, 100-byte names, multibyte UTF-8, tiny files.
- Output order is Java's: UTF-16 code units (`TreeMap<String>`), not UTF-8
  bytes.

## Performance

`bench-200m.yml` generates 200M rows with the official 1BRC generator and runs
gruppera (with and without PGO) against the 1BRC winner on OpenJDK and as a
GraalVM native image, on one runner, gating on byte-identical output:

```sh
gh workflow run bench-200m.yml --ref my-branch            # -f rows=100000000 for a quicker pass
```

Runners vary from run to run, so compare contenders within one run, not
across runs. Locally, `python3 bench.py target/release/gruppera data/measurements.txt`
prints the best of five. Datasets stay out of git (`data/` and `*.txt` are
ignored).

## Releasing

1. Bump `version` in `Cargo.toml` and add a `CHANGELOG.md` entry on `main`.
2. Actions → release → Run workflow with that version. The run checks the
   version, builds every target, tags `v<version>`, creates the GitHub release
   and publishes to each registry that is switched on.

Registries are switched on per repository with Actions variables, once the
account and trusted publisher exist:

| variable | publishes to | trusted publisher to configure |
|---|---|---|
| `PUBLISH_CRATES=true` | crates.io | crate `gruppera`, workflow `release.yml` |
| `PUBLISH_NPM=true` | npm (`gruppera` + one package per platform) | each package, workflow `release.yml` |
| `PUBLISH_PYPI=true` | PyPI (platform wheels) | project `gruppera`, workflow `release.yml`, environment `release` |
| `HOMEBREW_TAP=owner/homebrew-tap` | Homebrew formula | secret `TAP_TOKEN` with write access to the tap |
| `SCOOP_BUCKET=owner/scoop-bucket` | Scoop manifest | the same `TAP_TOKEN` |

Re-running the workflow for a version that already exists rebuilds its files
and publishes it to any registry that doesn't have it yet.
