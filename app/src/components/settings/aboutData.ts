export const PROJECT_LINKS = {
  author: "https://github.com/kokoa-77",
  repo: "https://github.com/kokoa-77/LumiLauncher",
  releases: "https://github.com/kokoa-77/LumiLauncher/releases",
  license: "https://github.com/kokoa-77/LumiLauncher/blob/main/LICENSE",
  security: "https://github.com/kokoa-77/LumiLauncher/blob/main/SECURITY.md",
} as const;

export const AUTHOR = {
  name: "Kokoa77",
  handle: "@kokoa-77",
  url: PROJECT_LINKS.author,
} as const;

export const APP_LICENSE = {
  spdx: "GPL-3.0-only",
  copyright: "Copyright (C) 2026 kokoa-77 and Lumi Contributors",
} as const;

export type CreditId = "bmclapi" | "mcim" | "modrinth" | "curseforge";

export interface ServiceCredit {
  id: CreditId;
  name: string;
  url: string;
}

/** 关于页外部服务与社区致谢 */
export const SERVICE_CREDITS: readonly ServiceCredit[] = [
  { id: "bmclapi", name: "BMCLAPI", url: "https://bmclapidoc.bangbang93.com/" },
  { id: "mcim", name: "MCIM", url: "https://www.mcimirror.top/" },
  { id: "modrinth", name: "Modrinth", url: "https://modrinth.com/" },
  { id: "curseforge", name: "CurseForge", url: "https://www.curseforge.com/minecraft" },
];

export type LicenseCategory = "rust" | "frontend" | "reference";

export interface ThirdPartyEntry {
  category: LicenseCategory;
  name: string;
  license: string;
  usage: {
    zh: string;
    en: string;
  };
}

/** 关于页「第三方开源许可」清单：直接使用的开源库、参考与模板。新增直接依赖时在这里补一条 */
export const THIRD_PARTY_ENTRIES: readonly ThirdPartyEntry[] = [
  // Rust 核心与应用壳
  {
    category: "rust",
    name: "tauri & tauri-plugin-*",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "桌面应用框架、窗口状态、单实例、系统目录选择、外链打开与签名自动更新",
      en: "Desktop framework, window state, single instance, native dialogs, URL opener, and signed updater",
    },
  },
  {
    category: "rust",
    name: "specta & tauri-specta",
    license: "MIT",
    usage: {
      zh: "Rust 与 TypeScript 类型安全接口绑定自动生成",
      en: "Type-safe Rust-to-TypeScript command and event binding generation",
    },
  },
  {
    category: "rust",
    name: "tokio & tokio-util",
    license: "MIT",
    usage: {
      zh: "异步运行时与任务取消令牌",
      en: "Async runtime and cancellation tokens",
    },
  },
  {
    category: "rust",
    name: "reqwest & futures-util",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "基于 rustls 与 HTTP/2 的网络请求与并发流式下载",
      en: "HTTP/2 networking with rustls, system proxy support, and concurrent streaming downloads",
    },
  },
  {
    category: "rust",
    name: "serde & serde_json",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "结构化配置、元数据与接口数据序列化",
      en: "Structured serialization for configs, metadata, and API payloads",
    },
  },
  {
    category: "rust",
    name: "thiserror & anyhow",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "核心库与应用壳的错误建模与传播",
      en: "Structured error definitions and propagation",
    },
  },
  {
    category: "rust",
    name: "tracing & tracing-appender",
    license: "MIT",
    usage: {
      zh: "结构化脱敏日志与按日滚动写入",
      en: "Structured redacted logging with daily log rotation",
    },
  },
  {
    category: "rust",
    name: "sha1, sha2, md-5 & getrandom",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "下载完整性校验、OAuth PKCE 随机数与历史存档兼容",
      en: "Download integrity hashing, OAuth PKCE randomness, and legacy save compatibility",
    },
  },
  {
    category: "rust",
    name: "zip & flate2 (zlib-rs)",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "整合包导入导出、原生库解压、世界备份与 level.dat 读取",
      en: "Modpack archives, native extraction, world backups, and gzip level.dat decoding",
    },
  },
  {
    category: "rust",
    name: "image",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "截图与缩略图缩放、自动封面保存及桌面快捷方式图标生成",
      en: "Thumbnail scaling, automatic world covers, and desktop shortcut icon generation",
    },
  },
  {
    category: "rust",
    name: "keyring-core & windows-native-keyring-store",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "在 Windows 凭据管理器中按账号隔离保存 Microsoft 刷新令牌",
      en: "Per-account storage of Microsoft refresh tokens in Windows Credential Manager",
    },
  },
  {
    category: "rust",
    name: "rusqlite (bundled SQLite)",
    license: "MIT",
    usage: {
      zh: "只读解析 Modrinth App 的实例清单以导入实例",
      en: "Read-only inspection of Modrinth App instance databases during import",
    },
  },
  {
    category: "rust",
    name: "sysinfo, winreg & trash",
    license: "MIT",
    usage: {
      zh: "内存探测、注册表发现本机 Java 以及删除实例移入回收站",
      en: "Memory inspection, Windows registry Java discovery, and Recycle Bin deletion",
    },
  },
  {
    category: "rust",
    name: "xcap & win-screenshot",
    license: "Apache-2.0 / MIT",
    usage: {
      zh: "自动封面：检测游戏前台窗口并仅截取窗口内容区",
      en: "Automatic covers: detecting foreground game windows and capturing client-area frames",
    },
  },
  {
    category: "rust",
    name: "windows-sys & windows-version",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "Windows 版本识别（云母背景）与受控内存整理接口封装",
      en: "Windows version checks for Mica backdrop and controlled memory trim bindings",
    },
  },
  {
    category: "rust",
    name: "encoding_rs, regex, base64, uuid, clap & tempfile",
    license: "MIT / Apache-2.0 / BSD-3-Clause",
    usage: {
      zh: "GBK 日志解码、脱敏正则、PKCE 与皮肤编码、命令行与临时文件管理",
      en: "GBK log decoding, redaction patterns, PKCE/skin encoding, CLI, and temporary files",
    },
  },

  // 前端界面
  {
    category: "frontend",
    name: "react & react-dom",
    license: "MIT",
    usage: {
      zh: "界面组件与渲染框架",
      en: "User interface rendering library",
    },
  },
  {
    category: "frontend",
    name: "react-router",
    license: "MIT",
    usage: {
      zh: "页面路由与按需懒加载拆分",
      en: "Client-side routing and lazy route splitting",
    },
  },
  {
    category: "frontend",
    name: "motion",
    license: "MIT",
    usage: {
      zh: "弹簧过渡与共享封面转场动效",
      en: "Spring animations and shared cover transitions",
    },
  },
  {
    category: "frontend",
    name: "tailwindcss",
    license: "MIT",
    usage: {
      zh: "语义化设计令牌与样式系统",
      en: "Semantic design tokens and utility styling",
    },
  },
  {
    category: "frontend",
    name: "zustand",
    license: "MIT",
    usage: {
      zh: "前端状态管理与会话更新缓存",
      en: "Frontend state management and session update caching",
    },
  },
  {
    category: "frontend",
    name: "i18next & react-i18next",
    license: "MIT",
    usage: {
      zh: "简体中文 / 英文双语本地化",
      en: "Simplified Chinese and English localization",
    },
  },
  {
    category: "frontend",
    name: "lucide-react",
    license: "ISC",
    usage: {
      zh: "通用操作与状态图标",
      en: "General interface action and status icons",
    },
  },
  {
    category: "frontend",
    name: "@tanstack/react-virtual & sonner",
    license: "MIT",
    usage: {
      zh: "长列表虚拟滚动与轻量底部提示条",
      en: "Virtualized long lists and lightweight toast notifications",
    },
  },
  {
    category: "frontend",
    name: "marked & dompurify",
    license: "MIT / MPL-2.0 / Apache-2.0",
    usage: {
      zh: "项目详情与更新日志的 Markdown 渲染及严格 HTML 安全清洗（按需加载）",
      en: "On-demand Markdown rendering and strict HTML sanitization for project details and changelogs",
    },
  },
  {
    category: "frontend",
    name: "skinview3d (three)",
    license: "MIT",
    usage: {
      zh: "官方档案皮肤的 3D 预览（按需加载，流畅模式不加载）",
      en: "On-demand 3D preview for official skins (skipped in Lite visual mode)",
    },
  },
  {
    category: "frontend",
    name: "vite, typescript, vitest & @biomejs/biome",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "前端构建打包、类型检查、单元测试与代码规范（仅开发期）",
      en: "Frontend bundling, type checking, unit testing, and linting (development only)",
    },
  },

  // 思路参考与安装模板
  {
    category: "reference",
    name: "portablemc (Forge processor reference)",
    license: "Apache-2.0",
    usage: {
      zh: "仅参考 install_profile.json 结构与处理器占位符规则，代码全部独立重写",
      en: "Reference only for install_profile.json structure and processor placeholders; implemented from scratch",
    },
  },
  {
    category: "reference",
    name: "Tauri CLI NSIS template",
    license: "MIT / Apache-2.0",
    usage: {
      zh: "Windows NSIS 安装脚本衍生自 Tauri CLI 2.12.4 模板（保留 TAURI-LICENSE-MIT.txt）",
      en: "Windows NSIS installer script derived from Tauri CLI 2.12.4 template (preserves TAURI-LICENSE-MIT.txt)",
    },
  },
  {
    category: "reference",
    name: "HMCL (-DignoreList note)",
    license: "—",
    usage: {
      zh: "仅参考关于新版引导器按文件名匹配 -DignoreList 的说明注释，未复制任何代码",
      en: "Concept note only on modern bootstrapper -DignoreList filename matching; no code copied",
    },
  },
];
