# Rust CI/CD Design

## Goal

PR quality gate that blocks merging unless `cargo fmt`, `cargo clippy`, `cargo test`, and `cargo build --release` all pass for the Rust workspace. Also runs on pushes to `main` for continuous feedback.

## Triggers

- **PRs targeting `main`** when paths match: `cli/**`, `shared/**`, `server/**`, `Cargo.toml`, `Cargo.lock`, `.github/workflows/rust-ci.yml`
- **Pushes to `main`** with the same path filters

## Concurrency

- Group: `rust-ci-${{ github.ref }}`
- Cancel-in-progress: true (new commits cancel stale runs)

## Rust toolchain

- Add `rust-toolchain.toml` at workspace root, pinning stable with `rustfmt` and `clippy` components
- CI reads the toolchain file via `actions-rust-lang/setup-rust-toolchain` so local and CI versions stay in sync

## Jobs

All run on `ubuntu-latest`.

### check (fmt + clippy)

Steps:
1. `actions/checkout@v5`
2. `actions-rust-lang/setup-rust-toolchain` (reads `rust-toolchain.toml`)
3. Restore sccache + cargo registry caches
4. `cargo fmt --all --check`
5. `cargo clippy --all-targets --all-features -- -D warnings`

### test (needs: check)

Steps:
1. checkout → setup toolchain → restore caches
2. `cargo test --all-features`

### build (needs: check)

Steps:
1. checkout → setup toolchain → restore caches
2. `cargo build --release`
3. `actions/upload-artifact@v4` — upload `target/release/super` binary, 7-day retention

## Caching

- **sccache** for Rust compilation (cache key includes hash of `Cargo.lock`)
- **`~/.cargo/registry`** and **`~/.cargo/git`** for crate downloads
- Maven-like restore-keys fallback so partial cache hits still help

## Clippy strictness

- `-D warnings` — any clippy warning fails the build, no exceptions
