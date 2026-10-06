# Working on gruppera — guide for AI agents

gruppera is a 1BRC-style aggregator built on `unsafe` SWAR code. Speed only
counts when the output is byte-identical to the Java reference **for every
valid input** — the fuzz CI found a segfault on a 4096-byte file and a
sort-order mismatch that no benchmark dataset ever exercised.

## 1. Start with state

```sh
gh run list -R andrey-usa/gruppera -L 5        # ci (every push) + bench-200m (manual)
```

If `ci` is red on main, that's task #1.

## 2. Read CI through the checks API

Log and artifact downloads are usually blocked in agent sandboxes;
annotations are not:

```sh
R=andrey-usa/gruppera
JOB=$(gh api repos/$R/actions/runs/<run-id>/jobs -q '.jobs[0].id')
gh api repos/$R/actions/jobs/$JOB -q '.steps[] | "\(.name) = \(.conclusion)"'
gh api repos/$R/check-runs/$JOB/annotations -q '.[] | "[\(.title)] \(.message)"'
```

`bench-200m` publishes its results table as a `bench-200m results`
annotation.

## 3. Fast loop

- **Correctness locally, always:** `python3 tests/fuzz.py <binary> 120`.
  Even seeds end exactly on a 4 KiB page boundary, so any read past EOF
  faults instead of hitting zero padding. If crates.io is blocked, build a
  dependency-free copy (swap `memmap2::Mmap::map` for a raw `libc` mmap via
  `extern "C"`) and run `rustc -O -C target-cpu=native` on it — the engine
  logic has no other dependencies.
- **Performance in CI:** `gh workflow run bench-200m.yml --ref my-branch`
  (dispatch on a branch; `-f rows=100000000` for a quicker pass).
- One hypothesis per bench run, decided in advance; GitHub runners vary
  run to run, so compare within one run (it builds every contender on the
  same machine), not across runs.

## 4. Rules for `unsafe` changes

- Every load must provably stay inside the mapping, including the scalar
  tail path (`TAIL_MARGIN` covers only the vectorized workers).
- Any new parse/hash/compare path gets fuzz coverage at its boundaries
  (8/15/16/17-byte names, 100-byte names, multibyte UTF-8, tiny files).
- Output order is Java's: UTF-16 code units (`TreeMap<String>`), not
  UTF-8 bytes.

## 5. History and hygiene

- main history is curated (reset to a single commit on 2026-10-06). Work on
  a branch, squash before merging, no debug/probe commits on main. Experiment
  branches (`opt/*`) should be deleted or merged once concluded.
- Datasets stay out of git (`data/`, `*.txt` are ignored).
