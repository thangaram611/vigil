# Vigil — agent notes

Personal macOS tool. Scope and what was dropped: [`ROADMAP.md`](./ROADMAP.md). Do not start cross-OS, release, or packaging work.

## Verify

Cargo is the sole suite. It may not be on `PATH`: `export PATH="$HOME/.cargo/bin:$PATH"`.

```bash
cargo fmt --check
cargo build
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cargo test --features helper-test-seam   # only when touching the root-helper boundary
```

The LaunchAgent runs the install snapshot in `~/Library/Application Support/vigil/bin/`, not the dev build. After a code change, `vigil reload` — then check `vigil doctor`.

## Invariants

- **Chord rules live twice.** `validate_combo` (`crates/vigil/src/commands/lock.rs`) and `parse_chord` (`native/vigil-lock-helper/src/combo.rs`) are independent implementations. Change both in one commit or capture and unlock silently diverge.
- **The root helper is a privilege boundary.** It accepts only `engage`, `release`, `status`, and runs a fixed `pmset` argv. Never build a privileged command from request content.
- **Thermal and battery guards outrank everything**, including `vigil mode on`.
- **`status` and `doctor` are read-only.** They never write, refresh, or GC state.
- **Tests that spawn CPU-burning or long-lived children use `BoundedCpuHog`** ([`docs/testing.md`](./docs/testing.md)).

## Workflow

Single-user repo: commit to `main`, no feature branches. Latest crate versions, no pins.
