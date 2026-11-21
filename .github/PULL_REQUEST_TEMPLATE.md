<!--
Thanks for contributing! Code changes need an issue discussed with the maintainer first; see CONTRIBUTING.md.
感谢贡献！代码改动需要先开 Issue 与维护者讨论确认，详见 CONTRIBUTING.md。
-->

## Related issue · 关联的 Issue

Closes #

## Summary · 改动说明

<!-- What does this PR change, and why? / 改了什么，为什么改？ -->

## How it was tested · 怎么测试的

<!-- Commands you ran, what you checked in `pnpm dev`, screenshots for UI changes / 跑了哪些命令、在 pnpm dev 里看了什么，界面改动请附截图 -->

## AI usage · AI 使用情况

- [ ] No AI tools were used to write this code · 没有使用 AI 工具编写代码
- [ ] AI tools were used for parts of it, and I have reviewed and understand every line · 部分使用了 AI 工具，我已逐行检查并理解每一行

## Checklist · 提交自查

- [ ] The change was discussed in an issue first (typo and translation fixes excepted) · 已先在 Issue 中讨论（修错别字、翻译除外）
- [ ] `pnpm check` passes locally · 本地 `pnpm check` 全部通过
- [ ] `app/src/lib/bindings.ts` is regenerated if a command or event changed · 改了命令或事件时已重新生成 `bindings.ts`
- [ ] UI text is added to both `zh-CN.json` and `en.json` · 界面文字已同时加入 `zh-CN.json` 和 `en.json`
- [ ] Styling uses semantic tokens from `tokens.css` (no color literals) · 样式只用 `tokens.css` 的语义令牌
- [ ] No new `unsafe` and no `unwrap()` / `expect()` in non-test Rust code · 没有新增 `unsafe`，非测试代码没有 `unwrap()` / `expect()`
- [ ] Compliant with the Minecraft EULA and Usage Guidelines (no auth bypass, no CDN bypass, no telemetry) · 符合 Minecraft EULA 与使用准则
