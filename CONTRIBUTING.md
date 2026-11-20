# Contributing to Lumi · 参与贡献

Thank you for your interest in contributing to **Lumi**!

## Development Setup

1. Install [Rust](https://www.rust-lang.org/) (the exact toolchain is pinned in `rust-toolchain.toml`), MSVC C++ Build Tools (Windows SDK), [Node.js](https://nodejs.org/) 24+, and [pnpm](https://pnpm.io/) 10+.
2. Install frontend dependencies:
   ```powershell
   pnpm install --frozen-lockfile
   ```
3. Run the desktop app locally:
   ```powershell
   pnpm dev
   ```

## Architecture & Engineering Rules

Please review [ARCHITECTURE.md](ARCHITECTURE.md) before making substantial changes:

- **Strict Layering**: `crates/core` must stay free of Tauri dependencies so core logic remains testable and reusable by `crates/cli`.
- **Typed IPC Only**: Expose commands via `#[tauri::command]` + `#[specta::specta]`, then regenerate `app/src/lib/bindings.ts` with `cargo test -p launcher-app`. Never call raw `invoke("...")` from UI components.
- **Zero Business `unsafe` / `unwrap()`**: Keep `crates/core` and `app/src-tauri` free of `unsafe` (Win32 FFI lives exclusively in `crates/winmem`) and avoid `unwrap()` / `expect()` in non-test Rust code.
- **Design Tokens & i18n**: Use semantic CSS tokens in `app/src/styles/tokens.css` (no hardcoded hex/rgba literals in components) and keep both `app/src/locales/zh-CN.json` and `app/src/locales/en-US.json` synchronized.
- **Performance & Motion**: Respect reduced-motion / fluent performance mode and keep the initial static JS bundle within budget.

## Required Checks Before Opening a PR

All pull requests must pass:

```powershell
pnpm check
```

This runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, Biome lint, TypeScript typecheck, and Vitest unit tests.

## Compliance Boundaries

To protect players and comply with Minecraft's EULA and Usage Guidelines, we **do not accept** contributions that:

- Bypass official Microsoft / Minecraft: Java Edition ownership verification (no offline play, external auth servers, or auth injectors).
- Bypass CurseForge creator third-party download restrictions or guess/scrape restricted CDN URLs.
- Add unofficial capes, telemetry, tracking, or advertisements.

## Licensing

By submitting a pull request, you agree that your contributions will be licensed under `GPL-3.0-only` (see [LICENSE](LICENSE)).
