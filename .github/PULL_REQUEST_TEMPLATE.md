## Summary · 变更概述

<!-- Briefly describe what this PR changes and why / 简述本次改动内容与原因 -->

## Checklist · 提交自查

- [ ] `pnpm check` passes locally (`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, Biome, TypeScript, Vitest)
- [ ] `app/src/lib/bindings.ts` is updated if any Tauri command or event signature changed
- [ ] UI text uses i18n keys in both `zh-CN.json` and `en-US.json`
- [ ] Styling uses semantic tokens from `tokens.css` (no hardcoded color literals)
- [ ] No new `unsafe` in `crates/core` or `app/src-tauri`, and no business `unwrap()` / `expect()`
- [ ] Compliant with Minecraft EULA / Usage Guidelines (no auth bypass, no CDN bypass, no telemetry)
