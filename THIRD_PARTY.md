# 第三方依赖、外部服务与开源许可

本项目的源代码全部自行编写，采用 `GPL-3.0-only` 许可证发布（附 GPL 第 7 条品牌与名称保护附加条款，详见根目录 [LICENSE](LICENSE)）。下面是项目直接使用的开源库、设计参考与外部服务登记清单。

## 移植或参考的代码

目前没有移植任何第三方代码。以后如果移植 MIT / Apache 许可的实现片段，必须在这里登记：来源仓库、提交、文件、许可证。

只读过思路、代码自行重写的：

| 我们的文件 | 参考了什么 | 许可 |
|---|---|---|
| `crates/core/src/install/processors.rs` | portablemc 的 `portablemc/src/forge/` 模块：安装器里 `install_profile.json` 的结构、处理器参数的占位符规则、从 jar 清单读主类 | Apache-2.0 |
| `crates/core/src/launch/args.rs` 里的 `ensure_jar_ignored` | HMCL 的一段代码注释：新版引导器按文件名匹配 `-DignoreList`，所以要把原版 jar 的文件名补进去（只看了思路说明，未复制实现代码） | — |
| `app/src-tauri/windows/installer.nsi` | 衍生自 Tauri CLI 2.12.4 的 NSIS 安装器模板（保留原许可文件 `app/src-tauri/windows/TAURI-LICENSE-MIT.txt`） | MIT 或 Apache-2.0 |

## Rust 核心与应用壳

| 库 | 用途 | 许可 |
|---|---|---|
| tauri、tauri-build、tauri-plugin-opener / single-instance / window-state / dialog / updater | 桌面应用框架、窗口状态、单实例、系统目录选择、系统浏览器打开链接与签名自动更新 | MIT 或 Apache-2.0 |
| specta、specta-typescript、tauri-specta | 前后端类型安全绑定自动生成 | MIT |
| tokio、tokio-util | 异步运行时、取消令牌 | MIT |
| serde、serde_json | 序列化与配置读写 | MIT 或 Apache-2.0 |
| thiserror、anyhow | 错误处理 | MIT 或 Apache-2.0 |
| tracing、tracing-subscriber、tracing-appender | 结构化日志与按日滚动写入 | MIT |
| regex | 日志脱敏规则、版本规则匹配 | MIT 或 Apache-2.0 |
| reqwest（rustls、HTTP/2、跟随系统代理） | 网络请求与流式下载 | MIT 或 Apache-2.0 |
| futures-util | 并发流与下载调度 | MIT 或 Apache-2.0 |
| sha1、sha2 | 文件完整性校验与 OAuth PKCE 摘要 | MIT 或 Apache-2.0 |
| uuid | 任务 ID、临时文件名 | MIT 或 Apache-2.0 |
| md-5 | 兼容读取历史存档与旧离线 UUID 计算 | MIT 或 Apache-2.0 |
| flate2（zlib-rs） | 解压世界存档 `level.dat`（gzip） | MIT 或 Apache-2.0 |
| zip（纯 Rust + zlib-rs） | 解压原生库、导入与导出整合包、世界备份打包 | MIT |
| encoding_rs | 游戏输出不是 UTF-8 时按 GBK 解码 | (MIT 或 Apache-2.0) 且 BSD-3-Clause |
| winreg（仅 Windows） | 从注册表发现本机 Java | MIT |
| sysinfo（仅 system 功能） | 读取电脑总内存与空闲内存（自动分配内存与启动预检） | MIT |
| image（png、jpeg、ico） | 缩略图与截图缩放、自动封面保存、桌面快捷方式图标生成 | MIT 或 Apache-2.0 |
| clap | 命令行工具 `mcl` 参数解析 | MIT 或 Apache-2.0 |
| windows-version、windows-sys（仅 Windows） | 判断 Windows 版本（云母背景）与受控内存整理接口封装 | MIT 或 Apache-2.0 |
| xcap（仅 Windows，不含录屏功能） | 自动封面：检测游戏窗口是否处于前台且未最小化 | Apache-2.0 |
| win-screenshot（仅 Windows） | 自动封面：通过系统 `PrintWindow` 截取游戏内容区 | MIT |
| trash（仅 Windows） | 删除实例时移入系统回收站而非永久删除 | MIT |
| rusqlite（bundled） | 只读解析 Modrinth App 的 SQLite 实例清单（导入实例） | MIT（SQLite 为公有领域） |
| getrandom | 读取系统安全随机源生成 Microsoft OAuth 的 PKCE 与状态值 | MIT 或 Apache-2.0 |
| keyring-core、windows-native-keyring-store（仅 Windows） | 将 Microsoft 刷新令牌按账号隔离保存在 Windows 凭据管理器 | MIT 或 Apache-2.0 |
| base64 | 官方皮肤纹理数据与 OAuth PKCE 编码 | MIT 或 Apache-2.0 |
| tempfile | 启动时暂存 Lumi 的标准输入管道入口类与测试临时目录 | MIT 或 Apache-2.0 |

## 前端界面

| 库 | 用途 | 许可 |
|---|---|---|
| react、react-dom | 界面框架 | MIT |
| react-router | 页面路由与按需懒加载 | MIT |
| motion | 弹簧动效与共享元素转场 | MIT |
| tailwindcss | 语义化设计令牌与样式系统 | MIT |
| zustand | 前端状态管理 | MIT |
| i18next、react-i18next | 简体中文 / 英文多语言本地化 | MIT |
| lucide-react | 通用操作图标 | ISC |
| sonner | 右下角轻量提示条 | MIT |
| @tanstack/react-virtual | 长列表虚拟滚动（日志、模组列表、资源中心） | MIT |
| marked（锁定 18.1.0） | 资源中心详情页与更新日志的 Markdown 渲染（按需加载） | MIT |
| dompurify（锁定 3.4.17） | 清洗富文本 HTML：去除脚本、iframe、表单与事件属性，外链仅允许 https | MPL-2.0 或 Apache-2.0 |
| skinview3d（锁定 3.4.2，含 three 与 skinview-utils） | 官方档案皮肤的 3D 预览（按需加载，流畅模式不加载） | MIT |
| @tauri-apps/api、@tauri-apps/plugin-opener | 调用后端命令与通过系统浏览器打开外部链接 | MIT 或 Apache-2.0 |
| vite、typescript、vitest、@biomejs/biome（开发用） | 构建打包、类型检查、单元测试与代码规范检查 | MIT 或 Apache-2.0 |

## 外部服务与社区致谢

Lumi 所有元数据、游戏文件、账号认证与内容均以官方源为准，并感谢以下开源社区与镜像服务为中文玩家及模组生态提供的支持（Lumi 与下列服务均无隶属关系）：

| 服务 | 用途与接入原则 | 主页 |
|---|---|---|
| **BMCLAPI**（bangbang93） | Minecraft 游戏元数据、游戏文件、模组加载器与 Java 运行时的社区镜像；默认自动测速或官方失败时作为后备，可在设置中随时切回「仅官方源」 | <https://bmclapidoc.bangbang93.com/> |
| **MCIM**（mcmod-info-mirror） | Modrinth 与 CurseForge 的国内加速镜像（已按要求登记 `LumiLauncher/<版本号>` UA）；默认仅在官方接口失败/过慢时回退并校验缓存新鲜度，文件始终优先从官方 CDN 下载，API Key 绝不发送给镜像 | <https://www.mcimirror.top/> |
| **Modrinth** | 开放的 Minecraft 模组、整合包、资源包与光影平台；提供项目检索、依赖解析、版本列表与更新检查 | <https://modrinth.com/> |
| **CurseForge** | Minecraft 模组与整合包平台；严格遵守作者的第三方下载限制，禁止第三方下载的文件走浏览器补件认领，绝不绕过或推导 CDN 地址 | <https://www.curseforge.com/minecraft> |
| **Microsoft / Mojang 官方服务** | 官方 Microsoft OAuth 2.0 授权（系统浏览器 + PKCE）、Xbox Live、Minecraft Java 版游玩权核验、官方档案与正版游戏资产分发；认证链路独立于任何内容镜像 | <https://www.minecraft.net/> |
