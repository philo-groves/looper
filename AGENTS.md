# Agent Instructions for Looper

This file provides rules for AI agents modifying this Rust codebase.

## 🦀 Rust Conventions
- Follow idiomatic Rust practices and the [Rust API Guidelines](https://rust-lang.github.io).
- Use `snake_case` for variables/functions, `PascalCase` for types/traits.
- Prefer explicit error handling with `Result<T, E>` and `anyhow::Result`.
- Avoid `unwrap()` in production code. Use `?` for propagation or `match`/`if let` for handling.
- Use `4 spaces` for indentation.

## 🏗️ Project Structure (Crates)
- `looper-harness`: Core agent runtime functionality
- `looper-terminal`: TUI for communicating with the agent
- `looper-web`: Next.js-based web interface (the only non-Rust package)
- `fiddlesticks`: Agent harness crate available locally (../fiddlesticks)

## ⚙️ Development Commands
- **Build**: `cargo build --workspace`
- **Check**: `cargo clippy --all-targets --all-features -- -D warnings`
- **Format**: `cargo fmt --all`
- **Test**: `cargo test --all-features`

## 🛡️ Rules & Restrictions
- **NEVER** ignore clippy warnings or lints.
- **ALWAYS** update `Cargo.toml` when adding dependencies.
- **NEVER** edit files in `generated/` folders.
- **ALWAYS** add doc comments (`///`) to public functions and structs.
- **Testing**: Add unit tests in the same file as the code or a `tests` module. Add integration tests to the `tests/` directory.
- **README/AGENTS**: Do not edit the README.md or AGENTS.md unless requested.
- **NO COMPATIBILITY**: There are no users of this repository yet, so there is no need for backwards compatibility when making changes. Breaking changes are allowed.

## 🚀 Workflows
- None yet