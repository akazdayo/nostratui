# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2021 terminal client built with Ratatui and Tokio. Application entry and terminal lifecycle code live in `src/main.rs`.

- `src/app.rs` and `src/app/`: application state, keyboard input, relay-event handling, content rendering models, and resource scheduling.
- `src/ui.rs` and `src/ui/`: screen composition, timeline/detail rendering, editor layout, and shared presentation helpers.
- `src/network.rs` and `src/network/`: relay subscriptions, command handling, NIP helpers, and bounded image decoding.
- `src/graphics.rs` and `src/graphics/`: terminal capability detection and Kitty image-cache management.

Unit tests stay beside their modules, typically in `tests.rs` or a nested `tests/` directory. The project currently has no separate asset directory; UI output is generated at runtime.

## Build, Test, and Development Commands

Enter the reproducible development environment with `nix develop`, or use an existing stable Rust toolchain.

- `cargo run` starts the client in read-only mode.
- `NOSTR_SECRET_KEY=nsec1... cargo run` enables publishing and reactions. Never commit this value.
- `cargo run -- --relay wss://relay.example.com` selects a relay; repeat `--relay` for multiple relays.
- `cargo check` performs a fast compile check.
- `cargo test` runs all unit tests.
- `cargo fmt -- --check` verifies formatting.
- `cargo clippy --all-targets -- -D warnings` rejects lint warnings across production and test targets.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting (four-space indentation). Follow Rust conventions: `snake_case` for modules, functions, and tests; `PascalCase` for types and enums; `SCREAMING_SNAKE_CASE` for constants. Keep modules responsibility-focused and avoid rebuilding large catch-all files. Prefer explicit error propagation with `anyhow::Result`; reserve `unwrap` and `expect` for tests or proven invariants.

## Testing Guidelines

Add focused unit tests with every behavior change, especially for NIP parsing, timeline state, Unicode layout, and image limits. Name tests as behavior statements, such as `incoming_notes_do_not_change_the_selected_event`. Use Ratatui's `TestBackend` for rendered UI assertions. No coverage percentage is mandated, but regressions should receive a test.

## Commit & Pull Request Guidelines

History follows Conventional Commit-style prefixes: `feat:`, `fix:`, `perf:`, `refactor:`, and `chore:`. Keep the subject concise and scoped to one change; Japanese or English is acceptable.

Pull requests should explain user-visible behavior, implementation boundaries, and verification commands. Link related issues when available. Include terminal screenshots for meaningful UI changes, and call out changes affecting relay traffic, secret handling, or image-download limits.

## Security & Configuration

Pass secret keys only through `NOSTR_SECRET_KEY`. Preserve HTTPS-only image fetching and existing download, dimension, allocation, concurrency, and cache limits when modifying media handling.
