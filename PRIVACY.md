# Privacy Policy · 隐私政策

*Last updated: 2026-11-20*

Lumi (`com.lumilauncher.lumi`) is an independent, open-source desktop launcher for Minecraft: Java Edition developed by `kokoa-77` and contributors. We built Lumi to be **strictly local-first with zero telemetry**.

---

## English

### 1. Data We Do NOT Collect
- Lumi does **not** collect telemetry, usage analytics, crash tracking, advertising identifiers, hardware fingerprints, or personal information.
- Lumi operates **no backend user database** and never sends your account credentials, world saves, mod lists, or settings to any Lumi-operated server.

### 2. Local Data Stored on Your Device
All launcher data stays on your own computer:
- **Account summary**: Microsoft account display name, official Minecraft Java Edition UUID, active account selection, and cached official skin preview images.
- **Authentication tokens**: OAuth 2.0 refresh tokens are stored per account in **Windows Credential Manager**. If the system credential store is unavailable, tokens are kept only in volatile memory for the current session and discarded on exit.
- **Game & launcher files**: Instances, saves, backups, settings (`settings.json`), and local rolling logs (retained for 7 days, with sensitive tokens automatically redacted to `<redacted>`).

### 3. External Network Connections
Lumi only connects to external services required for features you initiate:
- **Microsoft / Xbox Live / Mojang Official Services**: Used strictly for OAuth 2.0 Authorization Code + PKCE sign-in, Java Edition play entitlement verification, official Java profile retrieval, and official Minecraft asset/runtime downloads. Authentication traffic **never** passes through third-party download mirrors.
- **Modrinth & CurseForge**: Used when you browse, search, install, or check updates for mods, modpacks, resource packs, shaders, or maps.
- **Optional Download Mirrors (BMCLAPI / MCIM)**: Used only for public game assets or public mod metadata/files according to your download settings. Personal API keys (e.g., CurseForge key) and authentication requests are never sent to mirrors.
- **Optional Manual Log Sharing (`mclo.gs`)**: Only triggered when you explicitly click upload and confirm a second dialog. Logs are passed through automatic secret redaction before leaving your machine.
- **GitHub Releases (Auto-Update)**: Checked only when auto-update is enabled in official release builds to compare the latest launcher version.
- **Local Discord IPC (Optional, Off by Default)**: Communicates strictly over the local Windows named pipe (`\\.\pipe\discord-ipc-*`) with a locally running Discord client when you opt in.

### 4. Account Removal & Deletion
- Removing an account in Lumi immediately revokes its active launch authorization and deletes its refresh token from Windows Credential Manager while preserving your instances and world saves.
- Uninstalling Lumi or deleting its app data directory removes local launcher preferences and logs; uninstalling preserves your Minecraft game directory by default unless you choose otherwise.

### 5. Contact
For privacy or security inquiries, please open a private security advisory via GitHub Security Advisories on `kokoa-77/LumiLauncher`.

---

## 简体中文

### 1. 我们不收集任何数据
- Lumi **不收集**任何遥测数据、使用统计、自动崩溃上报、广告标识、硬件指纹或个人隐私信息。
- Lumi **没有自建用户后台服务器**，绝不会将你的账号凭据、世界存档、模组清单或个人设置上传至开发者服务器。

### 2. 仅保存在你本机的数据
所有启动器数据均存放于你自己的电脑上：
- **账号摘要**：显示名称、官方 Minecraft Java 版 UUID、当前选中账号以及缓存的官方皮肤预览图。
- **认证凭据**：OAuth 2.0 刷新令牌按账号隔离存放在 **Windows 凭据管理器**；若系统凭据库不可用，仅暂存于本次运行内存，退出即销毁。
- **启动器与游戏数据**：实例、世界存档、备份、本地偏好设置（`settings.json`）以及按天滚动的本地日志（保留 7 天，写入前自动脱敏为 `<redacted>`）。

### 3. 外部网络连接说明
Lumi 仅在你使用对应功能时连接必要的外部服务：
- **Microsoft / Xbox Live / Mojang 官方服务**：用于系统浏览器 OAuth 2.0 授权码 + PKCE 登录、Java 版游玩权核验、官方档案读取与官方游戏资产下载。**账号认证流量绝不经过任何第三方镜像。**
- **Modrinth 与 CurseForge**：用于搜索、安装、更新模组、整合包、材质包、光影与地图。
- **可选社区下载镜像（BMCLAPI / MCIM）**：按你在「内容与下载」中的设置用于加速公开游戏文件或公开模组元数据；你的个人 API Key 与账号认证绝不发送给镜像。
- **可选手动日志分享（`mclo.gs`）**：绝不自动上传，仅在你手动点击并二次确认后，先经过自动脱敏再上传日志。
- **GitHub Releases（自动更新）**：仅在启用自动更新的正式安装版中检查是否有新版 Lumi。
- **本机 Discord 活动状态（可选，默认关闭）**：仅在你手动开启后，通过本机命名管道（`\\.\pipe\discord-ipc-*`）与本机 Discord 客户端通信。

### 4. 注销账号与删除数据
- 在 Lumi 中移除账号会立即撤销该账号的启动授权，并从 Windows 凭据管理器中清除其刷新令牌，同时保留你的实例与世界存档。
- 卸载 Lumi 默认保留你的游戏目录与存档，仅清理程序本体；如需彻底清除偏好与日志，删除本机应用数据目录即可。

### 5. 联系方式
如有隐私或安全相关问题，请通过 `kokoa-77/LumiLauncher` 仓库的 **GitHub Security Advisories（私密安全报告）** 联系维护者。
