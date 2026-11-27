# Privacy Policy · 隐私政策

*Last updated: 2026-11-25*

**English** · [简体中文](#简体中文)

Lumi (`com.lumilauncher.lumi`) is an independent, open-source desktop launcher for Minecraft: Java Edition developed by `kokoa-77` and contributors. Lumi is **local-first and collects no telemetry**.

---

## English

### 1. Data We Do NOT Collect
- Lumi does **not** collect telemetry, usage analytics, crash tracking, advertising identifiers, hardware fingerprints, or personal information.
- Lumi operates **no backend user database** and does not send your account credentials, world saves, mod lists, or settings to any Lumi-operated server.

### 2. Local Data Stored on Your Device
All launcher data stays on your own computer:
- **Account summary**: Microsoft account display name, official Minecraft Java Edition UUID, active account selection, and cached official skin preview images.
- **Authentication tokens**: OAuth 2.0 refresh tokens are stored per account in **Windows Credential Manager**. If the system credential store is unavailable, tokens are kept only in volatile memory for the current session and discarded on exit.
- **Game & launcher files**: Instances, saves, backups, settings (`settings.json`), and local rolling logs (retained for 7 days, with sensitive tokens automatically redacted to `<redacted>`).

### 3. External Network Connections
Lumi only connects to external services required for features you initiate:
- **Microsoft / Xbox Live / Mojang Official Services**: Used for OAuth 2.0 Authorization Code + PKCE sign-in, Java Edition play entitlement verification, official Java profile retrieval, and official Minecraft asset/runtime downloads.
- **Mod loader sources (Fabric, Quilt, Forge, NeoForge, Legacy Fabric, Babric)**: Used to list loader versions and download loader files directly from each project's official repository (including NeoForge's official CDN edge `neoforged.forgecdn.net` alongside `maven.neoforged.net`, `meta.legacyfabric.net` / `maven.legacyfabric.net`, `meta.babric.glass-launcher.net` / `maven.glass-launcher.net`, and Maven Central `repo1.maven.org`) when you create or update an instance.
- **Modrinth & CurseForge**: Used when you browse, search, install, or check updates for mods, modpacks, resource packs, shaders, worlds, or servers directly from official platform endpoints (including CurseForge's official CDN edges `edge.forgecdn.net` and `mediafilez.forgecdn.net`).
- **DNS-over-HTTPS (on by default)**: Domain names of download sources are resolved over HTTPS (RFC 8484 binary queries) through pure-IP pools — Cloudflare (`1.0.0.1`, `1.1.1.1`) and Quad9 (`9.9.9.9`) for overseas official domains, and DNSPod (`1.12.12.12`) and AliDNS (`223.5.5.5`) for domestic/general domains. Only the domain names are sent, and Lumi falls back to your system DNS if they cannot be reached or when a local proxy TUN fake-IP is active. You can turn this off under **Settings → Use DoH for download domains**.
- **Network connectivity check**: Only when you click **Run check** under **Settings → Network diagnostics**, Lumi sends lightweight HEAD/GET requests (without account credentials) to official authentication, Minecraft, and mod platform endpoints to report reachability and latency.
- **Minecraft servers**: To show player counts, latency and icons, Lumi sends a standard server list ping directly to the servers on your home screen, in your server lists, or in Discover. Server addresses with SRV records are looked up through your system's DNS servers.
- **Optional Manual Log Sharing (`mclo.gs`)**: Only triggered when you explicitly click upload and confirm a second dialog. Logs are passed through automatic secret redaction before leaving your machine.
- **GitHub Releases (Updates)**: The installed version downloads the small `latest.json` file from GitHub Releases once at startup to see whether a newer Lumi exists. The portable version does not check for updates.
- **Local Discord IPC (Optional, Off by Default)**: Communicates strictly over the local Windows named pipe (`\\.\pipe\discord-ipc-*`) with a locally running Discord client when you opt in.
- **Optional Community Mirror Builds (`community-mirrors` feature, off in standard builds)**: Standard builds do not contain or connect to any third-party download mirrors. In builds explicitly compiled with community mirror support and enabled by you in Settings, public game/loader artifacts (and optionally failed Modrinth files) that carry an official cryptographic hash may be fetched from public community CDN mirrors or a custom HTTPS mirror you configure, with zero account tokens or CurseForge keys ever sent to mirrors.

### 4. Account Removal & Deletion
- Removing an account in Lumi immediately revokes its active launch authorization and deletes its refresh token from Windows Credential Manager while preserving your instances and world saves.
- Uninstalling Lumi or deleting its app data directory removes local launcher preferences and logs; uninstalling preserves your Minecraft game directory by default unless you choose otherwise.

### 5. Contact
For privacy, compliance, or security inquiries, contact the maintainer at **<lumilauncher@outlook.com>** or open a private security advisory via [GitHub Security Advisories](https://github.com/kokoa-77/LumiLauncher/security/advisories/new) on `kokoa-77/LumiLauncher`. General questions can also be asked in [GitHub Discussions](https://github.com/kokoa-77/LumiLauncher/discussions).

Every request Lumi makes identifies itself with the User-Agent `LumiLauncher/<version>`. Like any internet connection, the services above can see your IP address; their own privacy policies apply to the data they receive.

---

## 简体中文

### 1. 我们不收集任何数据
- Lumi **不收集**任何遥测数据、使用统计、自动崩溃上报、广告标识、硬件指纹或个人隐私信息。
- Lumi **没有自建用户后台服务器**，不会把你的账号凭据、世界存档、模组清单或个人设置上传到开发者的服务器。

### 2. 仅保存在你本机的数据
所有启动器数据均存放于你自己的电脑上：
- **账号摘要**：显示名称、官方 Minecraft Java 版 UUID、当前选中账号以及缓存的官方皮肤预览图。
- **认证凭据**：OAuth 2.0 刷新令牌按账号隔离存放在 **Windows 凭据管理器**；若系统凭据库不可用，仅暂存于本次运行内存，退出即销毁。
- **启动器与游戏数据**：实例、世界存档、备份、本地偏好设置（`settings.json`）以及按天滚动的本地日志（保留 7 天，写入前自动脱敏为 `<redacted>`）。

### 3. 外部网络连接说明
Lumi 仅在你使用对应功能时连接必要的外部服务：
- **Microsoft / Xbox Live / Mojang 官方服务**：用于系统浏览器 OAuth 2.0 授权码 + PKCE 登录、Java 版游玩权核验、官方档案读取与官方游戏资产下载。
- **模组加载器官方源（Fabric、Quilt、Forge、NeoForge、Legacy Fabric、Babric）**：新建或更新实例时，直接从各项目官方仓库列出加载器版本并下载加载器文件（含 NeoForge 官方互备 CDN `neoforged.forgecdn.net` 与 `maven.neoforged.net`、Legacy Fabric 官方源 `meta.legacyfabric.net` / `maven.legacyfabric.net`、Babric 官方源 `meta.babric.glass-launcher.net` / `maven.glass-launcher.net` 及 Maven Central `repo1.maven.org`）。
- **Modrinth 与 CurseForge**：直接通过官方平台接口搜索、安装、更新模组、整合包、材质包、光影、地图与服务器（含 CurseForge 官方 CDN 边缘 `edge.forgecdn.net` 与 `mediafilez.forgecdn.net`）。
- **DNS over HTTPS（默认开启）**：下载源域名默认通过 RFC 8484 纯 IP 加密池解析——海外官方域名走 Cloudflare（`1.0.0.1`、`1.1.1.1`）与 Quad9（`9.9.9.9`），国内与通用域名走 DNSPod（`1.12.12.12`）与阿里公共 DNS（`223.5.5.5`）。仅发送域名本身，节点不可达或检测到本地代理 TUN Fake-IP 时自动交还或回退系统 DNS。可在 **设置 → 用 DoH 解析下载域名** 中关闭。
- **网络连通性检测**：仅在你在 **设置 → 网络诊断** 中手动点击「开始检测」时，向官方认证、游戏源与模组平台发送极轻量的不带凭据探测请求，用于展示连通状态与延迟。
- **Minecraft 服务器**：为显示在线人数、延迟和图标，Lumi 会向首页、服务器列表或发现页中的服务器直接发送标准的服务器列表查询；带 SRV 记录的服务器地址通过系统 DNS 查询。
- **可选手动日志分享（`mclo.gs`）**：不会自动上传，只有在你手动点击并再次确认后，才会先脱敏再上传日志。
- **GitHub Releases（检查更新）**：安装版在每次启动时从 GitHub Releases 下载一次很小的 `latest.json`，查看是否有新版 Lumi；便携版不检查更新。
- **本机 Discord 活动状态（可选，默认关闭）**：仅在你手动开启后，通过本机命名管道（`\\.\pipe\discord-ipc-*`）与本机 Discord 客户端通信。
- **可选社区加速构建（仅开启 `community-mirrors` 编译特性，标准默认构建不包含）**：标准发布版本不包含也不会连接任何第三方镜像站。仅在明确编译启用该特性且你于设置中主动开启时，携带官方强哈希校验锚点的公开游戏/加载器文件（以及可选开启的 Modrinth 官方连败应急文件）会通过公开社区加速节点或你自填的 HTTPS 镜像根下载；账号登录凭据与 CurseForge 密钥永远只走官方，绝不发往任何镜像站。

### 4. 注销账号与删除数据
- 在 Lumi 中移除账号会立即撤销该账号的启动授权，并从 Windows 凭据管理器中清除其刷新令牌，同时保留你的实例与世界存档。
- 卸载 Lumi 默认保留你的游戏目录与存档，仅清理程序本体；如需彻底清除偏好与日志，删除本机应用数据目录即可。

### 5. 联系方式
如有隐私、合规或安全相关问题，可发送邮件至 **<lumilauncher@outlook.com>**，或通过 `kokoa-77/LumiLauncher` 仓库的 [GitHub Security Advisories（私密安全报告）](https://github.com/kokoa-77/LumiLauncher/security/advisories/new) 联系维护者；一般问题也可以在 [GitHub Discussions](https://github.com/kokoa-77/LumiLauncher/discussions) 提问。

Lumi 发出的每个请求都以 User-Agent `LumiLauncher/<版本号>` 标明身份。和任何网络连接一样，上述服务能看到你的 IP 地址；它们收到的数据适用各自的隐私政策。
