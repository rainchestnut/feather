# Contributing

Feather is an open-source, Rust-first CAD lightweight conversion project. It is intentionally built without commercial CAD SDK adapters or placeholder proprietary backends.

## Development Checks

Run the same checks used by CI before submitting changes:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo doc --workspace --no-deps
```

## Scope

- Keep import support explicit and capability-driven.
- Prefer open, readable visualization payloads and native open-source tessellation paths.
- Add bounded resource limits for new archive, geometry, tessellation, or assembly expansion paths.
- Add integration-style tests through public APIs or the CLI for every user-visible importer behavior.
- Do not add commercial adapter abstractions, dormant vendor SDK hooks, or test-only production branches.

## Test Fixtures

Small synthetic fixtures may live under `tests/fixtures`. Do not commit private customer CAD files or proprietary datasets. Fixtures should be minimal, documented by test names, and safe to redistribute.

## Documentation

When changing format support, update:

- `README.md`
- `docs/compatibility.md` when compatibility semantics change
- `docs/json_contracts.md` when public JSON contracts change
- `crates/feather-lite/src/capabilities.rs`
- related CLI or core integration tests
