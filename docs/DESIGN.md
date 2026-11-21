# Lumi 设计与工程规矩

> English summary: these are the design and engineering rules every change to Lumi must follow — layering, IPC contract, design tokens, motion, security, error handling, required checks and performance budgets. Pull requests that break them will not be merged. See [ARCHITECTURE.md](../ARCHITECTURE.md) for the code layout and [CONTRIBUTING.md](../CONTRIBUTING.md) for the workflow.

这份是给贡献者看的公开版规矩，和维护者内部的开发合同保持一致，章节编号也一样（代码注释里写的「第 5 节」「第 8 节第 6 条」就指这里）。和它冲突的临时想法，以本文为准。

## 1. 产品一句话与取舍准则

**一个「打开就有游戏可以点」的启动器**——从双击图标到进入游戏，点击次数最少。

任何新功能先问：**它让进游戏更快，还是更慢？** 更慢的不做，或者往后放。

## 2. 分层与依赖方向

```
app/src（React 界面）
   │  只通过 app/src/lib/bindings.ts 里自动生成的函数调用后端
   ▼
app/src-tauri（应用壳）── commands / events / 窗口 / 日志
   │  只调用 crates/core，不写业务逻辑
   ▼
crates/core（核心库，纯 Rust）── 也供 crates/cli（命令行调试工具）使用
```

| 从 | 到 | 允许 |
|---|---|---|
| 界面 | `@/lib/bindings` 生成的 `commands` / `events` | ✅ |
| 界面 | `invoke("字符串")` 直接调用 | ❌ |
| 应用壳 | `crates/core` | ✅ |
| `crates/core` | `tauri` 或任何界面相关的库 | ❌ **绝对禁止** |

核心库内部从底到顶（下层不许引用上层）：

```
fs / config / redact / jobs  ←  net  ←  meta  ←  java / auth  ←  install / content / modpack  ←  launch
```

## 3. 界面禁区

- 不引入 `@tauri-apps/plugin-fs`、`@tauri-apps/plugin-shell` 这类直接碰系统的插件
- 不拼文件路径，不接触 token。界面只认 **ID**（实例 ID、账户 ID、任务 ID），路径由核心库解析
- 外部链接只能用 `@tauri-apps/plugin-opener` 交给系统浏览器

## 4. 接口契约

- 命令名格式 `<域>_<动作>`，例如 `settings_get`、`jobs_cancel`（前端自动变成 `commands.settingsGet`）
- 会失败的命令返回 `Result<T, AppError>`。`AppError` 有三个字段：`code`（`<域>.<原因>`）、`message`（中文人话）、`detail`（已脱敏的技术细节）。英文界面按 `code` 在 `en.json` 的 `error.backend.*` 里找译文
- 前端统一用 `@/lib/api` 的 `unwrap()` 拆结果，失败会抛出 `CommandError`
- 新增或修改命令：核心函数、`commands.rs`、`lib.rs` 的登记、重新生成的 `bindings.ts` **在同一次提交里一起改**
- `bindings.ts` 是自动生成的，**禁止手改**；运行 `cargo test -p launcher-app` 重新生成
- 跨进程的数字一律用 `f64` / `u32`，不用 `u64` / `i64`（前端 JS 会丢精度）

## 5. 文案与令牌

### 文案

- 用户可见的文字全部走 i18n（`app/src/locales/zh-CN.json`、`en.json`，两份的键和占位符必须一致，有单测检查），key 格式 `<页面>.<区块>.<名字>`。日志和代码注释不受限；只在开发版出现的组件展示页（`app/src/dev/`）也不受限
- **提示文字**（设置项下面那行灰字、弹窗开头的说明、输入框里的示例）**以现有文案为准**：书面、简洁，一两句写清楚这一项管什么、什么时候需要调或出问题怎么办（例：「同时下载的文件数量。网络不稳定或频繁超时时可适当调低。」）；不用口语化的说法。新写的提示照现有文案的语气写
- 藏起来的内容鼠标停上去要能看到：「+N」用 `ui/HoverCard` 的 `MoreTag` 列出藏起来的那几个，可能被截断的名字、简介用 `Truncated`，本来就藏着的细节用 `HoverHint`；不用系统自带的 `title` 灰色小框

### 令牌

- **颜色、圆角、阴影、字体、动效**只能用 `app/src/styles/tokens.css` 里的令牌（通过 Tailwind 语义类：`bg-surface`、`text-fg-muted`、`border-line`、`bg-accent`……）
- 组件里不许出现颜色字面量（`#fff`、`rgb()`、`oklch()`），Tailwind 自带的调色板已被禁用
- 深浅色、云母背景切换只改变量，不改组件
- 浮在别的内容上面的层（下拉列表、弹窗、面板、胶囊、抽屉）一律用 `bg-overlay`（永远不透明），不用 `bg-surface`（云母模式下半透明，会透字）
- 主色只给「色相 + 彩度」，亮度由 `lib/color.ts` 按对比度算（主色上的字、主色字都要 ≥ 4.5）；换主色走 `lib/themeEngine.ts`
- 不用实时模糊（`backdrop-blur`、`filter: blur`）：老电脑上最吃力。浮层后面只压暗；字压在截图上靠渐变遮罩

### 圆角

只用分级档位：`rounded-row`（列表行、图标）、`rounded-control`（按钮、输入框、下拉框、分段选择）、`rounded-panel`（面板、卡片、菜单）、`rounded-dialog`（弹窗）、`rounded-cta`（一页一个的主按钮）、`rounded-icon`（图标按钮、头像）、`rounded-tag`（标签）。两条规则：

1. **能点的控件一律胶囊**，同一排的控件一样高
2. **装东西的是圆角矩形，越外层越圆**。图标小方块、提示框不是控件，不许用 `rounded-control`

### 颜色的六种角色

全站一样，不许一处一个样：

| 角色 | 样子 | 用在 |
|---|---|---|
| 选中（现在在哪） | 主色浅底 `bg-accent-soft` + 主色字 `text-accent` | 侧栏、页签、分段选择、列表和下拉里选中的一项、设置弹窗左侧、当前页，没有例外 |
| 移到（悬停、键盘移到） | 灰底 `bg-surface-hover` | 不用主色浅底；命令面板移到的那一行左边加一条主色竖线 |
| 一页只有一个的最重要按钮 | 实心主色 `bg-accent`（`CtaButton` 或 `Button` 的 primary） | 继续、开始游戏、新建实例 |
| 反复出现的操作 | 主色描边 + 主色字，悬停才铺浅底 | 每张卡片上的安装；实例卡片上的开始游戏用 `IconButton` 的 `card` 档 |
| 做完了（好消息） | 主色浅底 + 主色字 + 勾（已收藏是主色实心 ♥） | 已安装、已收藏、能装进这个实例；不许用灰色，也不许用红色 |
| 只是信息 | 灰色带细线 | 所有标签 |
| 危险 | 红色 | 删除、结束游戏 |

### 层次

- 层次靠底色分，不靠大阴影：窗口底色 `bg-bg` → 内容面板 `bg-surface-2` → 卡片和控件 `bg-surface`（加 `shadow-raised` 这一层很浅的阴影）。深色主题的底色是固定的夜空蓝（`--night-h`），不跟世界的颜色走
- 压在世界截图上的字和小胶囊用 `on-hero`、`hero-chip` 这套令牌，渐变遮罩用 `hero-scrim`，不许自己写白字黑底
- 云母背景下窗口底色压一层很淡的天色，不让侧栏、标题栏露出系统的中性灰

### 同类的东西放在同一个位置、长一个样子

- **页面容器**：所有页面的最外层都用 `lib/layout.ts` 的 `PAGE` / `PAGE_X`（最大宽度 1152、左右留白 32），页面最外层不许自己写 `max-w-*`
- **返回**：只用标题栏左上角那一组——「Lumi」（回首页）、后退 / 前进（鼠标侧键、Alt + ← / → 也行）、现在在哪（点前面一级跳回去）。页面里不放「← 去处」这种返回链接
- **能点的东西**鼠标移上去变成手（`global.css` 统一处理），禁用的照旧是箭头；自己用 `div` 做的能点的东西要自己加 `cursor-pointer`
- **列表页**：标题那一排放这一页最重要的按钮；下面一排是搜索和条件；视图切换叫「卡片 / 列表」，放在列表正上方那一排的最右
- **页签**（`ui/Tabs`）切「这一页看哪部分」，**分段选择**（`ui/Segmented`）切「条件」，两个不混用；页签工具栏右边的操作一律是带边框的次要按钮
- **字号**只用这几档：11（`text-2xs`，角标）、12（说明）、14（正文）、16、18（弹窗标题）、20（首页大按钮的字）、24（页面标题）、30（横幅标题）、56（`text-hero`，首页世界名）；不许写 `text-[13px]` 这种
- **区块标题**用 `ui/SectionTitle`：14 号中粗 + 灰色数字；面板里的分组小字是 12 号灰色
- **弹窗**右上角都有「×」（`ui/Dialog` 自带）；Esc、点背后照旧能关；弹窗叠弹窗时 Esc 只关最上面一层
- **窗口按钮**（最小化、最大化、关闭）：点得到的范围是整条标题栏高的方块，看得见的悬停是圆，关闭悬停是红色的圆
- **设置都是弹窗、同一个外壳**（`components/settings/SettingsShell`）：全局设置（标题栏齿轮 / Ctrl + ,）和实例设置（实例页横幅「开始游戏」左边的齿轮）。实例能单独设的每组不要开关：控件一直摆着，没改过时显示的就是全局现在的值，标题旁边是带地球的灰色「跟着全局」；改了任何一项就变成这个实例自己的，标签换成主色「✏ 本实例 ×」，点 × 改回跟着全局

## 6. 动效

- 曲线只有三条：`ease-out`（出现）、`ease-in`（消失）、`ease-in-out`（移动）；Motion 弹簧只有两档：`spring.gentle`、`spring.snappy`（`app/src/lib/motion.ts`）
- 时长只有五种：100（悬停 / 按下）、180（开关 / 切换）、220（弹层）、260（元素进出）、340（页面）毫秒，**上限 400 毫秒**
- 只动 `transform` 和 `opacity`；禁止 `transition: all`；禁止动宽高和位置
- 例外一：主色（「随实例变色」）。五个 `--accent-*` 变量在 `tokens.css` 里统一过渡 340 毫秒，只在「精美」且没开「减少动效」时生效。组件里不许自己写颜色过渡，换色只调 `lib/themeEngine.ts` 的 `applyAccent`
- 例外二：环境光效（背景光点、呼吸光、首页封面的极慢推近、进度条流光）。周期可以长于 400 毫秒，但只动 `transform` 和 `opacity`；一屏最多一处呼吸光、光点不超过 24 颗（首页有世界封面时底部最多叠 8 颗）；颜色只用 `--mote`、`--accent` 这类令牌；**流畅模式和「减少动效」下必须停住**。只用在引导、安装程序、首页这类「画面」里，不用在列表、设置这类干活的地方
- 界面轻音效：纯 Web Audio 合成，零第三方依赖、零网络，默认关闭，只在用户交互后激活
- 必须尊重「减少动效」（系统设置和应用设置都要生效）和「画面效果：流畅」（`data-fx="lite"`，老电脑自动打开）：过渡变短，位移和缩放类动画关掉只留淡入淡出；新做的大动画、大图层都要想好流畅模式下怎么退

## 7. 长列表

可能超过 30 项的列表必须虚拟滚动（`@tanstack/react-virtual`，见 `components/VirtualList.tsx`）。列表项里不放大阴影。

## 8. 安全（违反就不合并）

1. 界面不碰路径和密钥（见第 3 节）
2. 所有不可信路径（整合包内文件名、压缩包条目、网络返回的文件名）**只能经过 `launcher_core::fs::safe_join`**
3. 所有配置写入都用 `launcher_core::fs::write_json_atomic`（原子写）
4. 所有下载必须校验哈希和大小，校验失败删除重下
5. 日志一律经过脱敏（文件日志已自动脱敏；token 类的值用 `launcher_core::redact::Secret` 包起来）
6. 启动 Java、执行安装处理器时，参数用数组传，**绝不拼成一条命令字符串**
7. 元数据和文件都以官方为准（Mojang、各加载器、Modrinth、CurseForge）。镜像只用两家：BMCLAPI（Minecraft 元数据和文件）、MCIM（Modrinth / CurseForge 的接口和文件），只在官方失败、太慢或者用户自己选了「优先镜像」时用；MCIM 的缓存按 `checked_at` 看，太旧的先回官方核对。不用别家启动器自己的元数据服务器。CurseForge 只用平台给的下载地址（镜像只换域名），作者禁止第三方下载的走补件，**不推导、不猜 CDN 地址**；官方接口带自己的 Key，**Key 只发给 CurseForge 自己的域名，不发给镜像**
8. 不复制任何 GPL 项目或 PCL 的代码；移植 MIT / Apache 代码必须登记到 `THIRD_PARTY.md`
9. 游戏启动只接受 Lumi 自有 Microsoft 应用的在线 Java 版许可和同一账户的官方档案。桌面、快捷方式与命令行共用核心启动授权边界。不接受离线用户名、第三方认证服务器或认证注入器
10. 不写 `unsafe`（工作区 `unsafe_code = "forbid"`），系统接口一律用现成的安全封装库。唯一例外是 `crates/winmem`：每个 `unsafe` 块只放一个调用、必须写 `// SAFETY:` 说明为什么安全（clippy 强制）；只给应用壳和命令行工具用，核心库不依赖它

## 9. 错误处理

- Rust 非测试代码禁止 `unwrap()` / `expect()`（clippy 会报错）；确有必要时用 `#[allow]` 并写明理由
- 不许空 `catch` / 吞掉错误，至少写一行日志
- 前端兜住三处：全局错误、未处理的 Promise、渲染错误（`ErrorBoundary`），**不白屏**
- 给用户的错误必须是人话，能给「下一步」按钮的就给

## 10. 提交前必须通过

```powershell
pnpm check
```

依次跑：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`pnpm typecheck`、`pnpm lint`、`pnpm test`。改了界面还要 `pnpm --dir app build` 后跑 `pnpm check:bundle`。

改了核心库或命令后，要用 `pnpm dev` 实际启动一次；涉及游戏启动的改动要真的进一次游戏。

## 11. 依赖管理

- 新增依赖先过三问：有没有更轻的？是否活跃维护？许可是否和 `GPL-3.0-only` 兼容？
- 不许引入第二个组件库或动效库（动效只用 Motion）
- 预览版依赖（`specta`、`tauri-specta`、`specta-typescript`）必须用 `=` 锁定精确版本

## 12. 性能预算

冷启动到首屏 < 0.8 秒；点击反馈 < 100 毫秒；动画稳定 60 帧；首屏 JS（gzip 后）≤ 250 KiB（CI 里 `pnpm check:bundle` 把关）；首屏长任务（> 50 毫秒）≤ 1 次。路由按需加载，图标按需引入。
