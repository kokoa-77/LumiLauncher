# Privacy Policy · 隐私政策

*Last updated: 2026-11-23*

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
- **Microsoft / Xbox Live / Mojang Official Services**: Used for OAuth 2.0 Authorization Code + PKCE sign-in, Java Edition play entitlement verification, official Java profile retrieval, and official Minecraft asset/runtime downloads. Authentication traffic **does not** pass through third-party download mirrors.
- **Mod loader sources (Fabric, Quilt, Forge, NeoForge)**: Used to list loader versions and download loader files when you create or update an instance.
- **Modrinth & CurseForge**: Used when you browse, search, install, or check updates for mods, modpacks, resource packs, shaders, worlds, or servers.
- **Optional Download Mirrors (BMCLAPI / MCIM)**: Minecraft game files and metadata use official Mojang sources first by default; BMCLAPI / MCIM are used only as fallback on failure or if you switch download settings to Auto / Mirror. Personal API keys (e.g., CurseForge key) and authentication requests are never sent to mirrors.
- **DNS-over-HTTPS (on by default)**: Domain names of download sources are resolved over HTTPS, through Cloudflare (`1.1.1.1`) for official overseas sources and AliDNS (`223.5.5.5`) for mirrors in mainland China. Only the domain names are sent, and Lumi falls back to your system DNS if they cannot be reached. You can turn this off under **Settings → Use DoH for download domains**.
- **Download speed test**: Only if you switch a Minecraft download source from the default Official mode to Auto, Lumi briefly measures the official servers and BMCLAPI to pick the faster one.
- **Minecraft servers**: To show player counts, latency and icons, Lumi sends a standard server list ping directly to the servers on your home screen, in your server lists, or in Discover. Server addresses with SRV records are looked up through your system's DNS servers.
- **Optional Manual Log Sharing (`mclo.gs`)**: Only triggered when you explicitly click upload and confirm a second dialog. Logs are passed through automatic secret redaction before leaving your machine.
- **GitHub Releases (Updates)**: The installed version downloads the small `latest.json` file from GitHub Releases once at startup to see whether a newer Lumi exists. The portable version does not check for updates.
- **Local Discord IPC (Optional, Off by Default)**: Communicates strictly over the local Windows named pipe (`\\.\pipe\discord-ipc-*`) with a locally running Discord client when you opt in.

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
- **Microsoft / Xbox Live / Mojang 官方服务**：用于系统浏览器 OAuth 2.0 授权码 + PKCE 登录、Java 版游玩权核验、官方档案读取与官方游戏资产下载。**账号认证的请求不经过任何第三方镜像。**
- **模组加载器官方源（Fabric、Quilt、Forge、NeoForge）**：新建或更新实例时，用于列出加载器版本并下载加载器文件。
- **Modrinth 与 CurseForge**：用于搜索、安装、更新模组、整合包、材质包、光影、地图与服务器。
- **可选社区下载镜像（BMCLAPI / MCIM）**：Minecraft 游戏文件与元数据默认优先从 Mojang 官方源下载；仅在官方源连接失败，或你在「内容与下载」中主动改为「自动 / 优先镜像」时才使用社区镜像。你的个人 API Key 和账号认证请求绝不发给镜像。
- **DNS over HTTPS（默认开启）**：下载源的域名通过 HTTPS 解析：海外官方源使用 Cloudflare（`1.1.1.1`），国内镜像使用阿里公共 DNS（`223.5.5.5`）。只发送域名本身，无法连接时自动改用系统 DNS。可在 **设置 → 用 DoH 解析下载域名** 中关闭。
- **下载测速**：仅在你把下载源从默认的「优先官方」手动改为「自动」时，Lumi 才会短暂测试官方服务器与 BMCLAPI 的速度。
- **Minecraft 服务器**：为显示在线人数、延迟和图标，Lumi 会向首页、服务器列表或发现页中的服务器直接发送标准的服务器列表查询；带 SRV 记录的服务器地址通过系统 DNS 查询。
- **可选手动日志分享（`mclo.gs`）**：不会自动上传，只有在你手动点击并再次确认后，才会先脱敏再上传日志。
- **GitHub Releases（检查更新）**：安装版在每次启动时从 GitHub Releases 下载一次很小的 `latest.json`，查看是否有新版 Lumi；便携版不检查更新。
- **本机 Discord 活动状态（可选，默认关闭）**：仅在你手动开启后，通过本机命名管道（`\\.\pipe\discord-ipc-*`）与本机 Discord 客户端通信。

### 4. 注销账号与删除数据
- 在 Lumi 中移除账号会立即撤销该账号的启动授权，并从 Windows 凭据管理器中清除其刷新令牌，同时保留你的实例与世界存档。
- 卸载 Lumi 默认保留你的游戏目录与存档，仅清理程序本体；如需彻底清除偏好与日志，删除本机应用数据目录即可。

### 5. 联系方式
如有隐私、合规或安全相关问题，可发送邮件至 **<lumilauncher@outlook.com>**，或通过 `kokoa-77/LumiLauncher` 仓库的 [GitHub Security Advisories（私密安全报告）](https://github.com/kokoa-77/LumiLauncher/security/advisories/new) 联系维护者；一般问题也可以在 [GitHub Discussions](https://github.com/kokoa-77/LumiLauncher/discussions) 提问。

Lumi 发出的每个请求都以 User-Agent `LumiLauncher/<版本号>` 标明身份。和任何网络连接一样，上述服务能看到你的 IP 地址；它们收到的数据适用各自的隐私政策。
