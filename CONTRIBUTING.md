# Contributing to Lumi · 参与贡献

**English** · [简体中文](#简体中文)

---

## English

Thank you for wanting to help Lumi! Lumi is maintained by one person, so the most useful help is often not code at all.

### Ways to Help Without Writing Code

- **Report bugs.** Use the [Bug report](https://github.com/kokoa-77/LumiLauncher/issues/new?template=bug_report_en.yml) template and include the steps to reproduce.
- **Report game crashes.** Use the [Game crash](https://github.com/kokoa-77/LumiLauncher/issues/new?template=crash_report_en.yml) template. After a crash, Lumi offers **Export diagnostics** (a redacted summary, mod list and log); on the instance page you can also open the **Log** tab and use **Upload log** to get an mclo.gs link.
- **Share ideas.** Post them in [Discussions → Ideas](https://github.com/kokoa-77/LumiLauncher/discussions/categories/ideas). Ideas that many players support become feature requests.
- **Improve translations.** If an English or Chinese text in Lumi reads oddly, tell us in an issue where it appears and what it should say.
- **Help others** in Discussions, on Discord or in the QQ group.

### Before Opening an Issue

- Search [existing issues](https://github.com/kokoa-77/LumiLauncher/issues?q=is%3Aissue) first, including closed ones.
- Make sure you are on the [latest version](https://github.com/kokoa-77/LumiLauncher/releases).
- One problem per issue.
- English and Chinese are both welcome.
- Never post tokens, passwords or email addresses. Logs and diagnostics exported by Lumi are already redacted.
- Security vulnerabilities go through [private vulnerability reporting](https://github.com/kokoa-77/LumiLauncher/security/advisories/new), never in public issues.

### Contributing Code

Lumi follows a strict design and engineering contract, and every change is reviewed by hand. To avoid wasted work:

1. **Open an issue first** describing what you want to change and why, and wait for the maintainer to agree on the approach. Pull requests without a prior discussion may be closed without review.
2. **Keep it small.** One pull request should do one thing. Typo and translation fixes can skip step 1.
3. **Understand every line you submit.** You are responsible for explaining and maintaining your change; pull requests the author cannot explain will not be merged.
4. Read [ARCHITECTURE.md](ARCHITECTURE.md) (code layout) and [docs/DESIGN.md](docs/DESIGN.md) (rules for layering, design tokens, motion, copy, security and performance).

#### Development Setup

1. Install [Rust](https://www.rust-lang.org/) (the exact toolchain is pinned in `rust-toolchain.toml`), MSVC C++ Build Tools (Windows SDK), [Node.js](https://nodejs.org/) 24+ and [pnpm](https://pnpm.io/) 12+ (pinned via `packageManager` in `package.json`).
2. Install dependencies and start the app:
   ```powershell
   pnpm install --frozen-lockfile
   pnpm dev
   ```
3. To try the UI without compiling Rust, run `pnpm --dir app dev` and open `http://localhost:1420/preview.html` (a preview backed by fake data; nothing is really downloaded or launched).

#### Rules Highlights

- **Strict layering**: `crates/core` must stay free of Tauri dependencies so the core logic remains testable and reusable by `crates/cli`.
- **Typed IPC only**: expose commands with `#[tauri::command]` + `#[specta::specta]`, then regenerate `app/src/lib/bindings.ts` with `cargo test -p launcher-app`. Never call raw `invoke("...")` from UI components.
- **No `unsafe`, no `unwrap()`**: `crates/core` and `app/src-tauri` contain no `unsafe` (Win32 calls live only in `crates/winmem`), and non-test Rust code avoids `unwrap()` / `expect()`.
- **Design tokens and i18n**: use the semantic tokens in `app/src/styles/tokens.css` (no color literals in components) and keep `app/src/locales/zh-CN.json` and `app/src/locales/en.json` in sync.
- **Performance and motion**: respect reduced motion and Lightweight mode, and keep first-screen JS within 250 KiB gzipped (`pnpm --dir app build && pnpm check:bundle`).

#### Required Checks

All pull requests must pass:

```powershell
pnpm check
```

This runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, Biome lint, the TypeScript typecheck and Vitest. CI runs the same checks on every pull request.

### Out of Scope

To protect players and comply with the Minecraft EULA and Usage Guidelines, we **do not accept** contributions that:

- Bypass Microsoft / Minecraft: Java Edition ownership verification (offline play, external authentication servers, authentication injectors).
- Bypass CurseForge creators' third-party download restrictions, or guess or scrape restricted CDN URLs.
- Add unofficial capes, telemetry, tracking or advertisements.

### Code of Conduct and License

Everyone taking part in Lumi's spaces follows the [Code of Conduct](CODE_OF_CONDUCT.md). By submitting a pull request, you agree that your contribution is licensed under `GPL-3.0-only` (see [LICENSE](LICENSE)).

---

## 简体中文

感谢你愿意帮助 Lumi！Lumi 目前由一个人维护，所以最有用的帮助往往不是写代码。

### 不写代码也能帮忙

- **报告问题**：使用[问题反馈](https://github.com/kokoa-77/LumiLauncher/issues/new?template=bug_report_zh.yml)模板，写清复现步骤。
- **报告游戏崩溃**：使用[游戏崩溃](https://github.com/kokoa-77/LumiLauncher/issues/new?template=crash_report_zh.yml)模板。游戏崩溃后 Lumi 会提供 **导出诊断包**（脱敏后的摘要、模组清单和日志）；也可以在实例页的 **日志** 页签点 **上传日志**，得到 mclo.gs 链接。
- **提出想法**：发到 [Discussions → Ideas](https://github.com/kokoa-77/LumiLauncher/discussions/categories/ideas)。很多玩家支持的想法会转成功能建议。
- **改进翻译**：Lumi 里哪句中文或英文读着别扭，提个 Issue 说明在哪里、应该怎么写。
- **帮助其他玩家**：在 Discussions、QQ 群或 Discord 里回答问题。

### 提交 Issue 之前

- 先[搜索已有的 Issue](https://github.com/kokoa-77/LumiLauncher/issues?q=is%3Aissue)，包括已关闭的。
- 确认用的是[最新版本](https://github.com/kokoa-77/LumiLauncher/releases)。
- 一个 Issue 只说一个问题。
- 中文、英文都可以。
- 不要贴出令牌、密码或邮箱地址。Lumi 导出的日志和诊断包已经自动脱敏。
- 安全漏洞请走[私密安全报告](https://github.com/kokoa-77/LumiLauncher/security/advisories/new)，不要发在公开 Issue 里。

### 贡献代码

Lumi 有一套严格的设计与工程规矩，每一处改动都由维护者人工审阅。为了不让你白费功夫：

1. **先开 Issue**，说明想改什么、为什么改，等维护者确认方案后再动手。没有事先讨论的 PR 可能不经审阅直接关闭。
2. **改动尽量小**：一个 PR 只做一件事。修错别字、改翻译可以跳过第 1 步。
3. **你提交的每一行都要自己理解**：你需要能解释并维护自己的改动，作者说不清楚的 PR 不会合并。
4. 先读 [ARCHITECTURE.md](ARCHITECTURE.md)（代码结构）和 [docs/DESIGN.md](docs/DESIGN.md)（分层、设计令牌、动效、文案、安全与性能的规矩）。

#### 开发环境

1. 安装 [Rust](https://www.rust-lang.org/)（版本锁定在 `rust-toolchain.toml`）、MSVC C++ 生成工具（Windows SDK）、[Node.js](https://nodejs.org/) 24+ 和 [pnpm](https://pnpm.io/) 12+（版本锁定在 `package.json` 的 `packageManager`）。
2. 安装依赖并启动：
   ```powershell
   pnpm install --frozen-lockfile
   pnpm dev
   ```
3. 不想编译 Rust 时，运行 `pnpm --dir app dev`，打开 `http://localhost:1420/preview.html`：这是用假数据驱动的整套界面预览，不会真的下载或启动游戏。

#### 规矩要点

- **严格分层**：`crates/core` 不依赖 Tauri，核心逻辑能单独测试，也供 `crates/cli` 使用。
- **只用强类型接口**：命令用 `#[tauri::command]` + `#[specta::specta]` 导出，再用 `cargo test -p launcher-app` 重新生成 `app/src/lib/bindings.ts`；界面组件里不直接调用 `invoke("...")`。
- **不写 `unsafe`、不用 `unwrap()`**：`crates/core` 和 `app/src-tauri` 里没有 `unsafe`（Win32 调用只在 `crates/winmem`），非测试的 Rust 代码不用 `unwrap()` / `expect()`。
- **设计令牌与多语言**：样式只用 `app/src/styles/tokens.css` 里的语义令牌（组件里不写颜色字面量），`app/src/locales/zh-CN.json` 和 `app/src/locales/en.json` 保持同步。
- **性能与动效**：尊重「减少动效」和流畅模式，首屏 JS 压缩后不超过 250 KiB（`pnpm --dir app build && pnpm check:bundle`）。

#### 必须通过的检查

所有 PR 都必须通过：

```powershell
pnpm check
```

它会依次运行 `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、Biome、TypeScript 类型检查和 Vitest。每个 PR 在 CI 上也会跑同样的检查。

### 不接受的改动

为保护玩家并遵守 Minecraft EULA 与使用准则，以下贡献**不会被接受**：

- 绕过 Microsoft / Minecraft Java 版正版验证（离线游玩、第三方认证服务器、认证注入器）。
- 绕过 CurseForge 作者的第三方下载限制，或推导、抓取受限的 CDN 地址。
- 加入非官方披风、遥测、追踪或广告。

### 行为准则与许可

参与 Lumi 社区的每个人都需遵守[行为准则](CODE_OF_CONDUCT.md#简体中文)。提交 PR 即表示你同意自己的贡献以 `GPL-3.0-only` 授权（见 [LICENSE](LICENSE)）。
