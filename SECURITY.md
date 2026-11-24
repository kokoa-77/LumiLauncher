# Security Policy · 安全说明

**English** · [简体中文](#简体中文)

---

## English

### Supported Versions

Only the [latest release](https://github.com/kokoa-77/LumiLauncher/releases) receives security fixes. Please update before reporting.

### Reporting a Vulnerability

Please **do not** open a public issue, discussion or chat message containing exploitable details or anyone's credentials.

Report privately through **GitHub Private Vulnerability Reporting**: open the repository's **Security** tab and choose **Report a vulnerability**, or go directly to <https://github.com/kokoa-77/LumiLauncher/security/advisories/new>. You may also contact the maintainer by email at **<lumilauncher@outlook.com>**.

Please include:

- The Lumi version and Windows version
- What an attacker could do, and under which conditions
- Steps to reproduce or a proof of concept (redact any tokens or personal data)

We aim to acknowledge reports within 72 hours, will keep you updated while the issue is fixed, and will credit you in the release notes unless you prefer to stay anonymous. Please give us reasonable time to release a fix before disclosing the issue publicly.

Counterfeit "Lumi" builds that spread malware can be reported the same way. Other fake websites or groups can be reported in a normal issue (see [TRADEMARK.md](TRADEMARK.md)).

### Verifying Your Download

- Download Lumi only from [GitHub Releases](https://github.com/kokoa-77/LumiLauncher/releases). Each release includes `SHA256SUMS.txt`; compare it with the output of `Get-FileHash <file> -Algorithm SHA256`.
- The Windows installer and executable are not code-signed yet, so SmartScreen may show a warning. A matching checksum confirms the file is the one published by the release workflow.
- Automatic updates are signed. The installed version only accepts update packages signed with Lumi's updater key and verifies the signature before installing.

### Security Design

- **Official account and ownership verification**: Microsoft sign-in runs in the user's system browser using OAuth 2.0 Authorization Code + PKCE. Before creating a Minecraft process, the Rust backend verifies the active Java Edition entitlement and the matching official Java profile directly with official Microsoft, Xbox Live and Minecraft endpoints (never through download mirrors).
- **Credential isolation, nothing on the command line**:
  - Refresh tokens are stored per account in **Windows Credential Manager** (or only in memory for the current session if it is unavailable). The UI receives only non-sensitive account summaries and never accesses tokens.
  - Verified Minecraft session credentials are delivered to the Java game process over its **standard input (`stdin`) pipe** via `SessionPipe.java`. Session tokens do not appear in process command lines, environment variables, temporary argument files, UI events or diagnostic exports.
- **Minimal frontend capabilities**: the WebView frontend has no direct file system or shell access; every privileged operation goes through typed Rust commands with strict Tauri capabilities and a Content Security Policy.
- **Path safety and archive validation**: entries in external archives (`.mrpack`, CurseForge packs, worlds, resource packs) are checked for path traversal (`..`), absolute paths, symlinks and shortcuts, case-insensitive name collisions, and size and entry limits before extraction.
- **Verified downloads and atomic writes**: downloaded files are verified by cryptographic hash and size; configuration and metadata files are written atomically, with a backup if a file is found corrupted.
- **Automatic log redaction**: access tokens, refresh tokens, Bearer headers, JWTs and API keys are replaced with `<redacted>` before logs are written, diagnostics are exported, or a log is manually shared to `mclo.gs`.

---

## 简体中文

### 支持的版本

只有[最新版本](https://github.com/kokoa-77/LumiLauncher/releases)会收到安全修复，报告前请先更新。

### 报告安全漏洞

请**不要**在公开的 Issue、讨论区或聊天群中发布可被利用的漏洞细节或任何人的账号凭据。

请通过 **GitHub 私密漏洞报告**提交：打开仓库的 **Security** 页签，选择 **Report a vulnerability**，或直接访问 <https://github.com/kokoa-77/LumiLauncher/security/advisories/new>；也可以直接发邮件至 **<lumilauncher@outlook.com>** 联系维护者。

请尽量写明：

- Lumi 版本和 Windows 版本
- 攻击者能做到什么、需要什么条件
- 复现步骤或概念验证（请抹掉其中的令牌和个人信息）

我们会在 72 小时内确认收到，修复过程中保持沟通，并在发布说明中致谢（如你希望匿名则不署名）。请在修复版本发布前给我们合理的时间，再公开披露。

传播恶意程序的冒名「Lumi」也可以用同样的方式举报；其他冒名网站或群可以直接提交普通 Issue（见 [TRADEMARK.md](TRADEMARK.md#简体中文)）。

### 核对下载的文件

- 只从 [GitHub Releases](https://github.com/kokoa-77/LumiLauncher/releases) 下载 Lumi。每个版本都附带 `SHA256SUMS.txt`，可用 `Get-FileHash <文件> -Algorithm SHA256` 的输出与其核对。
- Windows 安装包和程序暂未进行代码签名，SmartScreen 可能弹出提醒；校验和一致即说明文件与发布工作流产出的完全相同。
- 自动更新带有签名：已安装的 Lumi 只接受用 Lumi 更新密钥签名的更新包，并在安装前校验签名。

### 安全设计

- **正版账号与游玩权核验**：Microsoft 登录通过系统浏览器执行 OAuth 2.0 授权码 + PKCE 流程；创建游戏进程前，由 Rust 后端直连官方 Microsoft、Xbox Live 与 Minecraft 服务核验当前账号的 Java 版游玩权与官方档案（认证请求绝不经过任何下载镜像）。
- **凭据隔离，命令行零暴露**：
  - 刷新令牌按账号隔离保存在 **Windows 凭据管理器**（不可用时仅存于当前会话内存）；前端界面只能读到非敏感的账号摘要，无法接触任何令牌。
  - 核验通过的 Minecraft 会话凭据通过 Java 子进程的**标准输入（`stdin`）管道**交付（`SessionPipe.java`），不会出现在进程命令行、环境变量、临时参数文件、前端事件或诊断包中。
- **最小前端权限与 CSP**：前端不挂载直接读写文件或执行命令的插件，所有系统操作都通过强类型 Rust 命令与最小权限白名单受控执行。
- **外部路径与压缩包校验**：整合包、地图与外部资源在解压前统一校验路径越界（`..`）、绝对路径、符号链接与快捷方式、Windows 大小写同名冲突以及体积和条目上限。
- **下载校验与原子写**：所有下载均校验哈希与大小；配置文件采用原子写，发现损坏时自动备份。
- **全链路自动脱敏**：access token、refresh token、Bearer、JWT、API Key 在写入日志、导出诊断包以及手动确认上传 `mclo.gs` 前，统一替换为 `<redacted>`。
