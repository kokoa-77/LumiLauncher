# 架构说明

> 给贡献者与代码审阅者参考：项目目录结构、各层职责边界，以及新增一个功能的固定步骤。贡献规范见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 目录

```
.
├─ Cargo.toml              Rust 工作区（核心库、命令行工具、应用壳）
├─ package.json            pnpm 工作区入口：pnpm dev / pnpm build / pnpm check
├─ crates/
│  ├─ core/                核心库（纯 Rust，不依赖 Tauri）
│  │  └─ src/
│  │     ├─ fs/            atomic.rs（原子写）、safe_path.rs（安全路径）
│  │     ├─ config/        settings.rs（设置读写、损坏恢复、格式迁移）
│  │     ├─ jobs.rs        任务注册表（去重、取消、进度节流、自动收尾）；每个任务记实例、来源、每一步、
│  │     │                 速度（只算网络收到的字节）和剩余时间（按完成量），下载历史存进应用数据目录的
│  │     │                 jobs-history.json（后台线程写，坏的条目单独跳过），带应用壳的「重试配方」
│  │     ├─ redact.rs      日志脱敏、Secret 包装
│  │     ├─ net/           http.rs（统一客户端，所有请求同一个 UA）、mirrors.rs（官方 → BMCLAPI 改写，四种下载源）、
│  │     │                 speed.rs（「自动」时测官方和 BMCLAPI 谁快）、doh.rs（DoH：海外官方源先问 Cloudflare、
│  │     │                 国内镜像先问阿里，代理软件假地址模式时让开）、
│  │     │                 download.rs 里模组文件官方 CDN 失败时补 MCIM 镜像地址、
│  │     │                 download.rs（并发、续传、校验、换源重试的下载引擎）
│  │     │                 status.rs：问我的世界服务器的状态（公开的服务器列表 Ping 协议：在线人数、延迟、版本、简介、图标）
│  │     │                 srv.rs：查 _minecraft._tcp SRV 记录（问电脑设置的 DNS 服务器，自己拼最小的 DNS 查询）
│  │     ├─ meta/          manifest.rs（版本清单）、version.rs（version json 与继承合并）、
│  │     │                 rules.rs（rules 判定）、maven.rs（Maven 坐标 → 路径）
│  │     ├─ paths.rs       GameDir：游戏目录布局——共用的 versions/ libraries/ assets/（Mojang 标准布局）+ instances/（所有外来名字都经过安全校验）
│  │     ├─ java.rs        Java 扫描（官方运行时、JAVA_HOME、PATH、注册表、手动添加的）与按版本选择，每个 Java 一个 ID
│  │     ├─ java/          runtime.rs：没有合适的 Java 时下载 Mojang 官方运行时（逐文件校验，放 <游戏目录>/.mcl-java/）、删除下载的
│  │     │                 prefs.rs：用户的选择（手动添加的、每个大版本的默认，应用数据目录的 java.json）、设置里的列表
│  │     ├─ auth/          Microsoft 浏览器授权（microsoft.rs）、Java 版游玩权与官方档案
│  │     │                 accounts.rs 保存正版账号摘要并清理旧账号，tokens.rs 隔离 Windows 凭据
│  │     │                 核心启动授权绑定当前账号、撤销代数与有效期
│  │     │                 skin.rs 缓存官方档案皮肤，旧皮肤文件可只读预览
│  │     ├─ process.rs     子进程公共设置（不弹窗口，参数按数组传）
│  │     ├─ install/       vanilla.rs：原版补全（依赖库、客户端、资源、本地库、日志配置）
│  │     │                 processors.rs：Forge 系安装器通用流程（库、处理器、产物检查、安装回执）
│  │     │                 neoforge.rs：NeoForge 版本列表、版本号对应、安装入口
│  │     │                 setup.rs：新建实例（装原版 / 加载器 → 补全文件 → 登记）
│  │     ├─ content/       mod.rs：实例内容列表、启用停用（模组改名、光影写 Iris 设置、材质包写 options.txt）、
│  │     │                 识别、检查更新、一键更新、删除、改动历史与撤销
│  │     │                 mirror.rs：Modrinth、CurseForge 接口的换源：按设置先问官方或 MCIM，官方失败 / 5 秒没回改问 MCIM，MCIM 缓存 checked_at 超过 7 天先回官方，CurseForge 的 Key 只发官方
│  │     │                 modrinth.rs：Modrinth 搜索、选版本、解析前置、装光影自动补 Iris、按哈希批量识别、批量查更新
│  │     │                 store.rs：内容仓库（按 SHA-1 存一份；替换或删除前先进仓库，所以能撤销）
│  │     │                 identify.rs：识别缓存（文件 → 哈希 → 名字 / 版本 / 图标），读模组和材质包自带的说明
│  │     │                 names.rs：中文名：自带的中文名表 data/zh-names.json（CC0，也记着两个平台确认是同一个项目的编号）、
│  │     │                 从模组自带的 zh_cn 语言文件认中文名（结果按哈希记在识别缓存里）、中文关键词查表
│  │     │                 snapshot.rs：「永不玩坏」的清单快照（能玩的状态 / 改动前）、崩溃后比较改动、一键放回
│  │     │                 journal.rs：改动日志：一次改好几个文件的操作先记下每一步和记录文件的原样，出错或被打断就退回改之前
│  │     │                 detail.rs：项目详情的共用形状（介绍原文、图片、链接），两个平台各自的接口在 modrinth.rs / curseforge.rs
│  │     │                 cache.rs：搜索结果、项目详情的短时缓存（10 分钟，只在内存里）
│  │     │                 favorites.rs：收藏夹（应用数据目录的 favorites.json，坏了备份后从空的开始）
│  │     │                 maps.rs：地图（CurseForge Worlds）：按实例版本挑文件、下载校验、安全解到 saves/
│  │     │                 （最浅的 level.dat 定存档根、safe_join、4 GB / 20 万条目上限、先解到临时目录再改名）
│  │     ├─ instance/      实例：instances/<实例名>/（实例文件夹就是游戏目录）+ <启动器代号>/instance.json（记着要启动的版本）；
│  │     │                 列表、登记、起名去重、分组和置顶、复制、移到回收站、老布局（versions/<实例名>/）自动搬家
│  │     │                 link.rs：挂上的别家版本：instance.json 里多一个 link（别家的 .minecraft、版本、游戏工作目录），
│  │     │                 版本和依赖库从那边读，移除只删 Lumi 这边的设置
│  │     ├─ external/      别家启动器：root.rs 挂上别家的 .minecraft（认官方 / PCL / HMCL、游戏版本、加载器、
│  │     │                 版本隔离，同步成实例，在电脑上找）；foreign.rs 从 Prism / MultiMC、CurseForge、ATLauncher 导入实例
│  │     │                 （复制游戏文件）；modrinth_app.rs 只读 Modrinth App 的数据库（SQLite）列出它的实例
│  │     ├─ shortcut.rs    桌面快捷方式：按公开格式自己写 .lnk（不调 PowerShell）、世界图标转圆角 .ico、读 --play 参数
│  │     ├─ modpack/       mrpack.rs：Modrinth 整合包检查（路径、链接、快捷方式、同名、域名、哈希）与导入
│  │     │                 remote.rs：从 Modrinth 列整合包能装的版本、下载整合包（只认 Modrinth CDN、必须有 SHA-1）
│  │     │                 ready.rs：导入前的准备情况（游戏本体、加载器、Java 缺不缺）
│  │     ├─ saves/         mod.rs：存档列表、世界信息、截图列表；nbt.rs：NBT 读写（只写自己读得懂的）；
│  │     │                 servers.rs：实例的多人游戏列表 servers.dat（读、加一个：地址校验、去重、原子写）
│  │     │                 backup.rs：世界备份（zip 在实例的 Lumi/backups/，指纹没变跳过、自动备份留几份、恢复先备当前再整个换上去）
│  │     │                 notes.rs：世界笔记（坐标、待办、随手记，Lumi/world-notes.json）
│  │     └─ launch/        args.rs（启动参数拼装，含 Quick Play：直接进存档或服务器）、log.rs（log4j XML / 纯文本日志解析）、
│  │                       session.rs（游戏状态机）、prepare.rs（启动前准备：加载器、补全、Java、内存、参数）、
│  │                       memory.rs（内存自动分配、设置里的内存条、启动前检查、谁占着内存）、
│  │                       jvm.rs（垃圾回收预设、自定义参数按引号拆成数组）、
│  │                       gpu.rs（高性能显卡：写 Windows 显卡设置，账本记着改过哪些、关掉时只撤回自己写的）、
│  │                       mod.rs（启动进程、读输出、可强制结束）
│  └─ tests/               集成测试：download.rs（本机模拟服务器）、version_json.rs（真实 1.21.1 数据）
│  └─ cli/                 mcl 命令行调试工具
│  └─ winmem/              系统内存与窗口句柄封装（全仓唯一允许少量受控 unsafe 的独立 crate）
├─ app/
│  ├─ src-tauri/           应用壳
│  │  ├─ src/lib.rs        启动流程、插件、命令登记、类型导出
│  │  ├─ src/commands.rs   应用、设置、任务的命令
│  │  ├─ src/game.rs       实例、启动、资源搜索（走缓存）、游戏目录、整合包导入的命令（后台任务 + 事件）
│  │  ├─ src/linked.rs     别家启动器：挂上的目录（应用数据目录的 linked-roots.json，启动时、换游戏目录后后台重新扫）、
│  │  │                    在电脑上找目录、从别家导入实例（复制 → 装游戏本体和加载器）
│  │  ├─ src/discover.rs   发现内容（原「资源中心」）：项目详情、收藏夹、地图搜索安装、服务器搜索 / 加入 / 列表的命令
│  │  ├─ src/downloads.rs  下载中心：下载历史、重试配方（失败、取消、中断的能按它再来一次）、测速、
│  │  │                    DoH 测试、补件盯的文件夹；整合包导入的临时目录按接着装的键留着，清空历史时一起清
│  │  ├─ src/worlds.rs     存档：一个实例的世界、每个实例最近的世界、截图、打开存档 / 截图文件夹
│  │  ├─ src/system.rs     内存（内存条、启动前检查、整理内存：以管理员身份带 --clean-memory 再开一次自己）、Java 列表、显卡
│  │  ├─ src/images.rs     本地图片协议（界面只拿编号；按 ?w= 缩小后再给）
│  │  ├─ src/sessions.rs   运行中的游戏会话（状态、最近 5000 行日志、结束开关）
│  │  ├─ src/events.rs     后端 → 前端事件（任务进度、会话状态、日志批量）
│  │  ├─ src/window.rs     云母背景、就绪后显示、超时兜底
│  │  ├─ src/logging.rs    日志（按天滚动、脱敏）
│  │  ├─ src/error.rs      统一错误格式 AppError
│  │  ├─ tauri.conf.json   窗口、CSP、打包
│  │  └─ capabilities/     界面权限白名单
│  └─ src/                 React 界面
│     ├─ lib/bindings.ts   ★ 自动生成的前后端接口（不要手改）
│     ├─ lib/              api.ts、report.ts、i18n.ts、motion.ts、theme.ts
│     ├─ styles/           tokens.css（设计令牌）、global.css
│     ├─ components/       ui/（自己的组件库）、标题栏、侧边栏、错误兜底、启动胶囊、日志抽屉、
│     │                    任务中心、新建实例入口（四选一，含从别的启动器导入）与向导、安装确认、导入 / 安装整合包、改动历史、虚拟滚动列表
│     ├─ layouts/          AppShell（整体布局 + 页面过渡）
│     ├─ components/discover/ 发现内容：结果卡片 / 行、三态安装按钮、收藏、「装到」实例胶囊、
│     │                    「装好了 · 进世界试试」、安装流程、介绍渲染（按需加载 marked + DOMPurify）
│     ├─ pages/            首页、实例、实例详情、资源（两种进法）、项目详情
│     ├─ stores/           Zustand 状态（app.ts 设置；game.ts 实例、会话、任务、日志，统一订阅后端事件）
│     └─ locales/          zh-CN.json、en-US.json
└─ scripts/                发布打包与端到端测试脚本
```

## 启动流程

1. Rust：注册插件（单实例 → 窗口状态 → 打开链接）→ setup：初始化日志 → 给主窗口加云母（Win11）→ 建任务注册表和事件转发 → 读设置 → 启动「5 秒没就绪就强制显示」的兜底
2. 窗口一开始是**隐藏**的（`visible: false`）
3. 前端：按系统深浅色设主题 → 调 `appInfo` + `settingsGet` → 渲染 → 两帧之后调 `appReady`
4. 后端收到 `appReady` 才显示窗口：用户看到的第一帧就是完整界面，没有白屏
5. 如果前端 5 秒都没报告就绪（比如脚本加载失败），后端强制显示窗口，并在日志里记一条警告

## 新增一个功能的固定步骤

以「新增一个命令」为例：

1. **核心库**：在 `crates/core` 里写功能和单元测试（不碰 Tauri）。需要导出给前端的类型加上
   `#[cfg_attr(feature = "specta", derive(specta::Type))]`
2. **命令行**（可选，推荐）：在 `crates/cli` 加一个子命令，先在命令行跑通
3. **命令**：在 `app/src-tauri/src/commands.rs` 写一个薄函数，加 `#[tauri::command]` 和 `#[specta::specta]`；
   会失败的返回 `Result<T, AppError>`，需要的话在 `error.rs` 加 `From` 转换
4. **登记**：加到 `lib.rs` 的 `collect_commands![...]`
5. **生成类型**：`cargo test -p launcher-app`，`app/src/lib/bindings.ts` 会自动更新
6. **界面**：通过 `commands.xxx()` 调用，用 `unwrap()` 拆结果，错误交给 `reportError`；文字进 `zh-CN.json`
7. **检查**：`pnpm check` 全部通过，再 `pnpm dev` 实际跑一次

新增事件同理：在 `events.rs` 定义 `#[derive(..., specta::Type, tauri_specta::Event)]` 的结构体，加到 `collect_events![...]`，前端用 `events.xxx.listen(...)`。

## 数据位置

| 数据 | 位置 |
|---|---|
| 设置 | `%APPDATA%\com.lumilauncher.lumi\settings.json`（定名前在 `com.unnamed.mclauncher`，第一次打开新版时整个文件夹改名过来，`app/src-tauri/src/app_dirs.rs`） |
| 日志 | `%LOCALAPPDATA%\com.lumilauncher.lumi\logs\launcher.<日期>.log`（保留 7 天）。同一个文件夹里还有 WebView2 的数据（界面记住的「卡片 / 列表」这类偏好） |
| 游戏目录 | 环境变量 `MCL_GAME_DIR`（开发用）> 设置里选的（「设置 → 游戏目录」，由后端弹系统窗口选择）> `%APPDATA%\com.lumilauncher.lumi\minecraft` |
| 实例 | `<游戏目录>\instances\<实例名>\`（实例文件夹就是游戏目录），本启动器的设置在其中的 `Lumi\instance.json`（实例信息，记着要启动的 `versions\` 里的版本）、`content.json`（本启动器装的内容来源）、`history.json`（改动历史，最多 100 条）、`journal.json`（正在做的改动；平时没有，留着说明上次没做完，启动时退回）、`backups/` + `backups.json`（世界备份）、`world-notes.json`（世界笔记） |
| 内容仓库 | `<游戏目录>\.mcl-store\`：`objects\<前两位>\<SHA-1>`（被替换、删除的文件）、`identity.json`（识别缓存）、`icons\`（从文件里读出的图标） |
| 临时文件 | `<游戏目录>\.mcl-tmp\`（和游戏目录同盘，装完删除；不用系统临时目录）。在线安装下载的整合包在 `.mcl-tmp\packs\`，装完就删，启动器启动时再清一次残留 |
| 窗口位置 | 窗口状态插件自动保存 |
