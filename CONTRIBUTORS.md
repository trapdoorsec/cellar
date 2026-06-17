# Contributing to cellar

PRs are welcome. This document covers the build, test, and release workflow so you can get started quickly.

## Prerequisites

- Rust toolchain (stable) — <https://rustup.rs>
- Linux: `libxkbcommon`, OpenGL/Mesa dev headers (required by eframe/egui)
- macOS: Xcode command-line tools
- Windows: MSVC toolchain

## Build

```sh
make build          # debug build
make release        # optimized release build
make run            # build and run the GUI
```

Or with cargo directly:

```sh
cargo build --release
```

The binary lands at `target/release/cellar`.

## Test

```sh
make test           # run the full test suite
make clippy         # lint (treats warnings as errors)
make fmt-check      # verify formatting
```

All three should pass before opening a PR. To auto-fix formatting:

```sh
make fmt
```

## Shipping / distribution

### cargo-dist (cross-platform installers)

```sh
make dist-plan      # preview what will be built
make dist           # build all distributable artifacts
```

This produces shell/powershell installers, Homebrew bottles, and MSI packages per the config in `dist-workspace.toml`.

### Platform packages

```sh
make deb            # .deb package (requires: cargo install cargo-deb)
make rpm            # .rpm package (requires: cargo install cargo-generate-rpm)
make pkg            # Arch Linux package (requires: makepkg)
```

### Manual install

```sh
make install        # installs release binary to /usr/local/bin
```

## Pull request checklist

1. `make test` passes.
2. `make clippy` is clean.
3. `make fmt-check` is clean.
4. If adding user-visible features, update `README.md`.
5. Keep commits focused — one logical change per commit.

## Project layout

```
src/
  main.rs       - entry point
  app.rs        - egui GUI and state
  backend.rs    - build orchestration and file staging
  iso.rs        - ISO 9660 writer with Joliet support
  hash.rs       - SHA-256 hashing worker
  manifest.rs   - research-mode manifest generation
```
