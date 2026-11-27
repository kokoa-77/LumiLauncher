<div align="center">
  <img src="docs/screenshots/social-preview.png" alt="Lumi Launcher 微光启动器" width="100%" />

  <h1>Lumi Launcher · 微光</h1>

  <p><strong>独立、轻快、开源的 Windows Minecraft Java 版启动器</strong></p>
  <p>点亮你的世界 · 打开就有游戏可以点</p>

  <p>
    <a href="README.md">English</a> · <strong>简体中文</strong> · <a href="https://lumilauncher.org/zh/">官方网站</a> · <a href="https://lumilauncher.org/zh/demo">浏览器在线试玩</a>
  </p>

  <p>
    <a href="https://github.com/kokoa-77/LumiLauncher/actions/workflows/ci.yml"><img src="https://github.com/kokoa-77/LumiLauncher/actions/workflows/ci.yml/badge.svg" alt="Lumi Desktop CI" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--only-blue.svg" alt="许可证: GPL-3.0-only" /></a>
    <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-0078D6.svg" alt="支持平台: Windows 10/11" />
    <a href="https://github.com/kokoa-77/LumiLauncher/releases"><img src="https://img.shields.io/github/v/release/kokoa-77/LumiLauncher?include_prereleases&label=release" alt="最新版本" /></a>
  </p>

  <p>
    <a href="https://qm.qq.com/q/vSHOPvOKt2"><img src="https://img.shields.io/badge/QQ%E7%BE%A4-%E5%8A%A0%E5%85%A5-12B7F5?logo=qq&logoColor=white" alt="QQ 群" /></a>
    <a href="https://pd.qq.com/s/Nr15r3slq"><img src="https://img.shields.io/badge/QQ%E9%A2%91%E9%81%93-%E5%8A%A0%E5%85%A5-12B7F5?logo=qq&logoColor=white" alt="QQ 频道" /></a>
    <a href="https://space.bilibili.com/25681030"><img src="https://img.shields.io/badge/B%E7%AB%99-%E5%BC%80%E5%8F%91%E8%80%85-00A1D6?logo=bilibili&logoColor=white" alt="开发者 B 站" /></a>
    <a href="https://discord.gg/vrAQ7HwUCw"><img src="https://img.shields.io/badge/Discord-%E5%8A%A0%E5%85%A5-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://afdian.com/a/lumilauncher"><img src="https://img.shields.io/badge/%E7%88%B1%E5%8F%91%E7%94%B5-%E8%B5%9E%E5%8A%A9-946CE6" alt="在爱发电赞助" /></a>
  </p>
</div>

---

> **独立声明**：Lumi 是独立第三方启动器，**并非 Minecraft 官方产品**，**与 Mojang Studios 及 Microsoft 无隶属关系，亦未获得其认可**。游玩需要合法拥有 Minecraft Java 版游玩权的 Microsoft 正版账户。

## 界面一览

<table>
  <tr>
    <td width="50%">
      <img src="docs/screenshots/02-instances-light.png" alt="浅色主题实例库与分组管理" width="100%" />
      <p align="center"><sub><strong>实例库（浅色主题）</strong> · 独立实例隔离、分组管理与外部启动器迁移</sub></p>
    </td>
    <td width="50%">
      <img src="docs/screenshots/03-instance-detail.png" alt="实例详情与模组内容管理" width="100%" />
      <p align="center"><sub><strong>实例内容管理</strong> · 模组检查更新、锁定版本与改动历史回滚</sub></p>
    </td>
  </tr>
  <tr>
    <td width="50%">
      <img src="docs/screenshots/04-discover.png" alt="内置 Modrinth 与 CurseForge 资源发现页" width="100%" />
      <p align="center"><sub><strong>统一资源发现</strong> · 内置 Modrinth &amp; CurseForge 模组、整合包、光影与地图</sub></p>
    </td>
    <td width="50%">
      <img src="docs/screenshots/05-instance-sync.png" alt="本机跨实例同步设置" width="100%" />
      <p align="center"><sub><strong>本机跨实例同步</strong> · 在本机同步按键、画面、服务器、通用包和快捷栏</sub></p>
    </td>
  </tr>
</table>

## 核心亮点

- **直达你的世界与多人服**：打开 Lumi，首页就是你最近玩的世界或常用的服务器；封面从游戏截图自动生成，点「继续」就能用 Quick Play 直接进入。
- **随实例变色**：每个世界和整合包都有自己的主色。支持 Windows 11 云母（Mica）背景、浅色 / 深色 / 跟随系统主题、减少动效、给老电脑用的流畅模式，以及可选的界面音效（默认关闭）。
- **改坏了能撤回**：每次改动与启动记录都会汇入可视化时间线，随时一键回滚；提供模组反向依赖图、孤儿前置清理与实例笔记；启动前本地预检缺少前置、重复 modId、加载器与 Java 兼容性；世界存档支持自动备份与恢复。
- **全版本与整合包全生命周期**：内置 Modrinth 与 CurseForge 资源发现，支持 `.mrpack`、CurseForge `.zip` 以及含 `patches/*.json` 补丁的 MultiMC / Prism `.zip` 导入（兼容高魔改整合包）；支持实例版本/加载器升级向导、三方比对无损整合包更新（保留自加模组与存档）、4 格式导出与隐私过滤；覆盖正式版、快照、愚人节及远古 Beta/Alpha，以及 Fabric、NeoForge、Forge（含 1.5.2–1.12.2 怀旧版）、Quilt、Legacy Fabric 与 Babric。
- **本机跨实例同步**：在本机的实例之间同步 6 类内容（`options.txt` 分 9 类可视化编辑、按键可直接录入；`servers.dat` 按 NBT 合并去重；共享材质包；共享光影；命令记录；创造模式物品栏），不经过云端，也不需要账号；第一次开启会先自动备份，单个实例可以按项排除。
- **主流目录布局与迁移**：采用 `instances/<实例名>/` 隔离布局并共享游戏本体；支持在「设置 → 游戏」直接关联官方 / PCL / HMCL 目录（不复制文件），或在「新建实例 → 从其他启动器导入」从 Prism / MultiMC、Modrinth App、CurseForge、ATLauncher 导入实例。
- **高速官方源下载引擎**：直连 Mojang、各加载器、Modrinth 与 CurseForge 官方源，支持大文件 HTTP Range 分段并发、自适应带宽调度、断点续传、SHA-1/SHA-256 校验、RFC 8484 DoH、网络代理实时监听（跟随系统 / 直连 / 自定义 HTTP·SOCKS5）以及任务驾驶舱。
- **账号工坊与实用工具**：内置官方 3D 皮肤与披风工坊和账户安全中心，自动识别电脑上的 Java 或一键安装官方 Java、内存分配建议、存储空间分析和闲置版本清理、启动前后命令钩子、自定义游戏窗口标题、可选 Discord 活动状态，以及确认后脱敏上传日志到 `mclo.gs`。

## 下载与安装

请从 **[官方网站](https://lumilauncher.org/zh/#download)** 或 **[GitHub Releases](https://github.com/kokoa-77/LumiLauncher/releases)** 下载最新版本，这两个是仅有的官方下载渠道。

| 文件 | 说明 |
|---|---|
| `LumiLauncher_<版本号>_x64-setup.exe` | **安装版（推荐）**。为当前用户安装，添加开始菜单快捷方式，并接收经过签名的自动更新。 |
| `LumiLauncher_<版本号>_x64-portable.zip` | **便携版**。解压到任意可写的文件夹后运行 `Lumi.exe`；设置和默认游戏目录保存在旁边的 `LumiData` 文件夹中，需要手动更新。 |
| `SHA256SUMS.txt` | 用于核对以上文件的校验和。 |

**系统要求**：Windows 10 或 11（64 位），以及 Microsoft Edge WebView2 运行时（Windows 11 和保持更新的 Windows 10 已自带）。无需另外安装 Java：Lumi 会识别电脑上已有的 Java，也可以从 Mojang 官方下载 Java 运行时。

**提示「Windows 已保护你的电脑」**：Windows 安装包暂未进行代码签名，首次运行时 Microsoft Defender SmartScreen 可能弹出提醒，点击 **更多信息 → 仍要运行** 即可。如需确认文件未被篡改，可以先核对校验和：

```powershell
Get-FileHash .\LumiLauncher_0.7.0_x64-setup.exe -Algorithm SHA256
```

输出的值应与同一发布页 `SHA256SUMS.txt` 中对应文件的那一行一致。

> **谨防冒名版本**：Lumi 只在[官方网站](https://lumilauncher.org/zh/)与 [GitHub Releases](https://github.com/kokoa-77/LumiLauncher/releases) 发布。其他下载站的转载、所谓「破解版」「高级版」以及任何索要 Microsoft 密码的程序都不是官方版本。官方渠道列表见 [TRADEMARK.md](TRADEMARK.md#简体中文)。

## 账号认证与正版核验

- **官方正版游玩权核验**：Lumi 的 Microsoft Entra 应用已通过 Mojang 官方 AppID 白名单审核。登录通过系统浏览器执行 **OAuth 2.1 授权码 + PKCE（S256）+ `form_post`** 流程（天然支持系统浏览器的 Passkey / Windows Hello，不启用设备码公共流），并在启动前直连 Microsoft / Xbox Live / Minecraft 官方服务核验 Java 版游玩权限与官方档案。
- **不绕过正版验证**：不提供离线用户名游玩、第三方皮肤站登录、认证注入器或非官方披风。
- **源码自行编译说明**：官方发布包在构建阶段注入官方 Microsoft Entra 客户端 ID，开箱即可登录正版游玩；从源码自行编译且未设置 `LUMI_MICROSOFT_CLIENT_ID` 的构建会保持登录入口关闭（可按下方说明配置你自己获批的 AppID）。

## 隐私与安全

Lumi **不收集遥测和行为统计，也没有广告**。

- 刷新令牌按账号隔离保存在 **Windows 凭据管理器**（DPAPI 用户隔离；系统凭据库不可用时仅存于当前会话内存）。
- 核验通过的 Minecraft 游戏会话凭据经由 Java 子进程**标准输入（`stdin`）管道**交付（`SessionPipe.java`），不会出现在系统进程命令行、环境变量或临时参数文件里。
- 普通日志、崩溃日志与导出的诊断包均统一经过自动脱敏处理（`<redacted>`）。
- 详细说明见 [PRIVACY.md](PRIVACY.md) 与 [SECURITY.md](SECURITY.md)。

## 常见问题

<details>
<summary><strong>支持离线账号或非正版账号吗？</strong></summary>

不支持。Lumi 只为拥有 Minecraft Java 版的 Microsoft 账号启动游戏，不提供离线用户名、第三方认证服务器或认证注入器。
</details>

<details>
<summary><strong>能继续使用原来的 <code>.minecraft</code>、PCL 或 HMCL 的目录吗？</strong></summary>

可以。官方、PCL、HMCL 的 `.minecraft` 目录可以在「设置 → 游戏」直接关联（不复制文件），也可以从 Prism Launcher / MultiMC、Modrinth App、CurseForge、ATLauncher 导入实例。移除关联只删 Lumi 这边的记录，原目录的文件不动。
</details>

<details>
<summary><strong>杀毒软件或 SmartScreen 报毒，安全吗？</strong></summary>

Windows 安装包暂未进行代码签名，可能触发 SmartScreen 和部分杀毒软件的提醒。请只从官方网站或 GitHub Releases 下载，并将文件的 SHA-256 与 `SHA256SUMS.txt` 核对；校验和一致时，可以向杀毒软件厂商报告误报。自动更新包带有签名，Lumi 会在安装前校验。
</details>

<details>
<summary><strong>我的数据保存在哪里？</strong></summary>

- 安装版：设置在 `%APPDATA%\com.lumilauncher.lumi`，日志在 `%LOCALAPPDATA%\com.lumilauncher.lumi\logs`。默认游戏目录是 `%APPDATA%\com.lumilauncher.lumi\minecraft`，在「设置 → 游戏目录」中换过的以你选择的为准。
- 便携版：全部保存在 `Lumi.exe` 旁边的 `LumiData` 文件夹。
- 登录凭据保存在 Windows 凭据管理器中，不会以明文文件保存。

完整列表见 [ARCHITECTURE.md](ARCHITECTURE.md#data-locations)。
</details>

<details>
<summary><strong>怎样彻底卸载 Lumi？</strong></summary>

1. 先在 Lumi 中移除账号（同时会从 Windows 凭据管理器中删除对应的凭据）。
2. 在 **设置 → 应用 → 安装的应用** 中卸载 Lumi。游戏目录和存档会保留。
3. 如需删除全部数据，再删除 `%APPDATA%\com.lumilauncher.lumi` 和 `%LOCALAPPDATA%\com.lumilauncher.lumi`。**使用默认游戏目录时请先备份存档**，因为它就在第一个文件夹里。
</details>

<details>
<summary><strong>会有 macOS 或 Linux 版本吗？</strong></summary>

目前只支持 Windows 10 和 11，其他平台以后再考虑。
</details>

## 社区与支持

| 渠道 | 适合做什么 |
|---|---|
| [GitHub Issues](https://github.com/kokoa-77/LumiLauncher/issues/new/choose) | 报告问题、游戏崩溃和功能建议（中文、英文都可以） |
| [GitHub Discussions](https://github.com/kokoa-77/LumiLauncher/discussions) | 交流想法、提问、分享你的配置 |
| [QQ 群](https://qm.qq.com/q/vSHOPvOKt2) · [QQ 频道](https://pd.qq.com/s/Nr15r3slq) | 中文玩家交流、版本公告与使用求助 |
| [开发者 B 站](https://space.bilibili.com/25681030) | 开发进度与视频 |
| [Discord](https://discord.gg/vrAQ7HwUCw) | 与其他玩家和开发者交流，中文、英文分区 |
| `lumilauncher@outlook.com` | 联系维护者、合规与审核沟通 |

遇到问题请尽量在 GitHub 上提交，只在群里反馈容易被后续消息淹没。安全漏洞请走[私密安全报告](https://github.com/kokoa-77/LumiLauncher/security/advisories/new)，不要公开发布。

## 赞助

Lumi 的所有功能对所有人免费。如果它帮到了你，欢迎在[爱发电](https://afdian.com/a/lumilauncher)支持开发。赞助是自愿的，不会换来额外功能、抢先版或优先处理问题。

## 参与贡献

报告问题、提供崩溃日志、翻译和提出想法都是很有价值的贡献，不会写代码也能帮上忙。如果想贡献代码，请先开 Issue 说明想法，等维护者确认方案后再动手。开始之前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 从源码构建

### 环境要求（Windows 10/11 x64）

- [Rust](https://www.rust-lang.org/)（版本锁定于 [`rust-toolchain.toml`](rust-toolchain.toml)）与 MSVC C++ 生成工具（Windows 10/11 SDK）
- [Node.js](https://nodejs.org/) 24+ 与 [pnpm](https://pnpm.io/) 12+（版本锁定在 `package.json` 的 `packageManager`）
- Microsoft Edge WebView2 运行时（Windows 11 及现代 Windows 10 已内置）

### 常用命令

```powershell
# 安装前端锁定依赖
pnpm install --frozen-lockfile

# 运行全套静态与测试校验（cargo fmt / clippy / test + Biome / TypeScript / Vitest）
pnpm check

# 启动桌面开发模式
pnpm dev

# 构建 Windows 安装程序（NSIS）
pnpm --dir app tauri build --bundles nsis
```

如需使用你自己的已获批 Microsoft Entra 应用 Client ID 构建：

```powershell
$env:LUMI_MICROSOFT_CLIENT_ID = "<你的-client-id>"
pnpm --dir app tauri build --bundles nsis
```

了解代码结构请阅读 [ARCHITECTURE.md](ARCHITECTURE.md)，设计与工程规矩见 [docs/DESIGN.md](docs/DESIGN.md)，参与贡献请先查看 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 发布文件的可信性

- 官方版本由本仓库的 [Windows 发布工作流](.github/workflows/windows-release.yml) 在 GitHub Actions 上构建，每个版本都附带 `SHA256SUMS.txt`。
- **Windows 安装包和程序暂未进行代码签名**，可能出现 SmartScreen 提醒，请按[下载与安装](#下载与安装)中的方法核对校验和。
- **自动更新带有签名**：已安装的 Lumi 只接受用 Lumi 更新密钥签名的更新包，并在安装前校验签名。

## Fork、转载与品牌

你可以依照 **`GPL-3.0-only`**（含 [LICENSE](LICENSE) 中的第 7 条附加条款）学习、Fork、修改和再分发 Lumi 的源代码。简单来说，修改版必须：

1. 使用**自己的名称、应用 ID 和图标**，不得使用 `Lumi`、`Lumi Launcher`、`微光` 或 `com.lumilauncher.lumi`。
2. 明确说明**不是官方 Lumi**，也未获得 Lumi 项目认可。
3. **不得沿用 Lumi 的凭据**（Microsoft Entra Client ID、更新密钥、各类 API Key）。
4. 以 `GPL-3.0-only` 继续开源，并保留原有的版权声明。

转载未经修改的官方版本时，文件须与发布页完全一致。完整规则、官方渠道列表和举报冒名版本的方式见 **[TRADEMARK.md](TRADEMARK.md#简体中文)**。

## 致谢与开源许可

- Lumi 直连 **Mojang / Microsoft**、**[Modrinth](https://modrinth.com/)** 与 **[CurseForge](https://www.curseforge.com/minecraft)** 官方服务。感谢维护这些服务的每一个人。
- Lumi 使用的开源库及其许可，可在应用的 **设置 → 关于 → 第三方开源组件与许可** 中查看。
- **源代码**：采用 **GNU General Public License v3.0 only（`GPL-3.0-only`）** 开源（附第 7 条品牌与名称保护附加条款），详见 [LICENSE](LICENSE)。
- **品牌与原创美术资产**：Copyright © 2026 `kokoa-77`，随代码以 `GPL-3.0-only` 发布；修改版如何使用 Lumi 名称、图标与插画受第 7 条附加条款限制。
