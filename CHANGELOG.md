# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.4.0] - 2026-10-09

First public release.

### Added

- Memory-mapped, multi-threaded aggregation of 1BRC-format input with
  byte-identical output to the Java reference (UTF-16 key order, Java
  rounding).
- `--version` and `--help`; a missing input file prints usage and exits 1.
- Prebuilt binaries for Linux (x86_64, aarch64; static), macOS (arm64,
  x86_64) and Windows (x64); packages for crates.io, npm, PyPI, Homebrew and
  Scoop.

[0.4.0]: https://github.com/andrey-usa/gruppera/releases/tag/v0.4.0
