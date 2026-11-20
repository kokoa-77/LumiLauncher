# Security Policy · 安全说明

## Security Design Guarantees (English)

- **Official Account & Ownership Verification**: Microsoft sign-in runs in the user's system browser using OAuth 2.0 Authorization Code + PKCE. Before creating a Minecraft process, the Rust backend verifies active Java Edition play entitlement and the matching official Java profile directly against official Microsoft, Xbox Live, and Minecraft endpoints (never routed through download mirrors).
- **Credential Isolation & Zero Command-Line Exposure**:
  - Refresh tokens are stored per account in **Windows Credential Manager** (or volatile session memory if unavailable). The frontend UI only receives non-sensitive account summaries and never accesses tokens.
  - Verified Minecraft session credentials are delivered to the Java game process over its **standard input (`stdin`) pipe** via `SessionPipe.java`. Session tokens are omitted from Windows process command lines, environment variables, temporary argument files, UI events, and diagnostic exports.
- **Minimal Frontend Capabilities**: The WebView frontend has no direct filesystem or shell plugin access; all privileged operations go through typed Rust commands with strict Tauri capabilities and CSP rules.
- **Path Safety & Archive Validation**: External archive entries (`.mrpack`, CurseForge packs, maps, resource packs) are validated against path traversal (`..`), absolute paths, symlinks/shortcuts, case-insensitive collisions, and size/entry limits before extraction.
- **Verified Downloads & Atomic Writes**: Downloaded artifacts are verified by cryptographic hash and size; configuration and metadata files use atomic write-and-rename with corruption backup.
- **Automatic Log Redaction**: Access tokens, refresh tokens, Bearer headers, JWTs, and API keys are redacted to `<redacted>` before writing logs, exporting diagnostics, or manually sharing to `mclo.gs`.

### Reporting a Vulnerability

Please **do not** open a public GitHub issue containing exploitable details or personal credentials.

Use **GitHub Private Vulnerability Reporting** (`Security` → `Advisories` → `Report a vulnerability`) on `kokoa-77/LumiLauncher` to report security issues privately. We aim to acknowledge reports within 72 hours.

---

## 安全设计保障（简体中文）

- **正版账号与游玩权核验**：Microsoft 登录通过系统浏览器执行 OAuth 2.0 授权码 + PKCE 流程；创建游戏进程前由 Rust 后端直连官方 Microsoft、Xbox Live 与 Minecraft 服务核验当前账号的 Java 版游玩权与官方档案（认证请求绝不走任何下载镜像）。
- **凭据隔离与进程命令行零暴露**：
  - 刷新令牌按账号隔离保存在 **Windows 凭据管理器**（不可用时仅存于当前会话内存）；前端界面只读非敏感账号摘要，无法接触任何令牌。
  - 核验通过的 Minecraft 会话凭据通过 Java 子进程的**标准输入（`stdin`）管道**交付（`SessionPipe.java`），不会出现在系统进程命令行、环境变量、临时参数文件、前端事件或诊断导出包中。
- **最小前端权限与 CSP**：前端不挂载直接读写文件或执行命令的插件；所有系统操作均通过强类型 Rust 命令与最小权限白名单受控执行。
- **不信任外部路径与压缩包校验**：整合包、地图与外部资源在解压前统一校验路径越界（`..`）、绝对路径、符号链接/快捷方式、Windows 大小写同名冲突与体积上限。
- **下载校验与原子写**：所有下载均校验哈希与大小；配置文件采用原子写并在损坏时自动备份恢复。
- **全链路自动脱敏**：access token、refresh token、Bearer、JWT、API Key 在写入日志、导出诊断包及手动确认上传 `mclo.gs` 前统一替换为 `<redacted>`。

### 报告安全漏洞

请**不要**在公开 Issue 中披露可被利用的漏洞细节或任何账号凭据。

请通过 `kokoa-77/LumiLauncher` 仓库的 **Security → Advisories → Report a vulnerability**（私密安全报告通道）提交，我们会在 72 小时内响应。
