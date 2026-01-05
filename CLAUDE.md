# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build                    # Build all crates
cargo build -p ecsdb_web       # Build web crate only
cargo test                     # Run all tests
cargo test -p ecsdb            # Run core library tests
cargo test <test_name>         # Run a specific test
```

## Architecture

**ecsdb** is a SQLite-backed Entity-Component-System database with a web interface. Three workspace crates:

- **`ecsdb`** — Core ECS library. Entities are integer IDs with named components stored in a single SQLite `components(entity, component, data)` table. Components use storage strategies: `JsonStorage` (default, serialized as JSON text), `BlobStorage` (raw bytes), `NullStorage` (markers). The query system converts type-safe Rust filters (`With`, `Without`, `AnyOf`, `Or`) into SQL via an intermediate representation.

- **`ecsdb_web`** — Web UI built on Axum + Maud (compile-time HTML templates) + htmx. The `service()` function in `lib.rs` creates a Tower service. Request routing is in `ecs_service.rs` via pattern matching on path segments (not Axum Router). The server detects htmx requests via `HX-Request` header and returns partial HTML fragments vs full pages. CSS is provided by `missing.css` (classless framework). Static assets (`htmx.js`, `missing.css`) are embedded via `include_bytes!`.

- **`ecsdb_derive`** — Proc macros: `#[derive(Component)]`, `#[derive(Resource)]`, `#[derive(Bundle)]`. Supports `#[component(storage = "json|blob|null")]` and `#[component(name = "...")]` attributes.

## Key Patterns

- `Ecs::open(path)` / `Ecs::open_in_memory()` opens a database. Schema and migrations run automatically.
- `CreatedAt` and `LastUpdated` timestamps are managed by SQLite triggers, not application code.
- The web layer takes an `open_db` closure to get an `Ecs` instance per request.
- Maud's `html!` macro is used for all HTML generation — templates are Rust code, not separate files.
- Rust edition 2024.
