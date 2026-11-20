/**
 * Minecraft 原版 `options.txt` 完整 9 栏可视化配置目录与 LWJGL 按键映射（D26c）。
 * 仅被懒加载的 `SyncEditors` 引用，不占首屏体积。
 */

export type OptionsTabId =
  | "skin"
  | "video"
  | "language"
  | "audio"
  | "controls"
  | "keybinds"
  | "chat"
  | "accessibility"
  | "custom";

export interface BiText {
  zh: string;
  en: string;
}

export interface SelectChoice {
  value: string;
  label: BiText;
}

export type OptionControl =
  | {
      kind: "bool";
      trueVal?: string;
      falseVal?: string;
    }
  | {
      kind: "select";
      choices: SelectChoice[];
    }
  | {
      kind: "slider";
      min: number;
      max: number;
      step: number;
      toUi: (raw: string | undefined) => number;
      fromUi: (ui: number) => string;
      format: (ui: number, isZh: boolean) => string;
    };

export interface McOptionSpec {
  key: string;
  tab: Exclude<OptionsTabId, "keybinds" | "custom">;
  label: BiText;
  hint: BiText;
  defaultValue: string;
  control: OptionControl;
}

export interface KeybindSpec {
  key: string;
  label: BiText;
  defaultValue: string;
}

function parseFloatOr(raw: string | undefined, fallback: number): number {
  if (raw === undefined) return fallback;
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function parseIntOr(raw: string | undefined, fallback: number): number {
  if (raw === undefined) return fallback;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function pctSlider(defaultRatio: number, min = 0, max = 100, step = 1, scale = 100): OptionControl {
  return {
    kind: "slider",
    min,
    max,
    step,
    toUi: (raw) =>
      Math.min(max, Math.max(min, Math.round(parseFloatOr(raw, defaultRatio) * scale))),
    fromUi: (ui) => (ui / scale).toFixed(2),
    format: (ui) => `${ui}%`,
  };
}

export const MC_OPTIONS_CATALOG: readonly McOptionSpec[] = [
  // 1. 皮肤自定义
  {
    key: "mainHand",
    tab: "skin",
    label: { zh: "主手", en: "Main hand" },
    hint: { zh: "选择角色主手为左手还是右手。", en: "Choose left or right as your main hand." },
    defaultValue: '"right"',
    control: {
      kind: "select",
      choices: [
        { value: '"right"', label: { zh: "右", en: "Right" } },
        { value: '"left"', label: { zh: "左", en: "Left" } },
      ],
    },
  },
  {
    key: "modelPart_cape",
    tab: "skin",
    label: { zh: "披风", en: "Cape" },
    hint: { zh: "显示玩家的披风（包括鞘翅纹理）。", en: "Show player cape and elytra texture." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_hat",
    tab: "skin",
    label: { zh: "帽子", en: "Hat" },
    hint: { zh: "显示皮肤头部外层。", en: "Show the outer hat skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_jacket",
    tab: "skin",
    label: { zh: "夹克", en: "Jacket" },
    hint: { zh: "显示皮肤躯干外层。", en: "Show the outer jacket skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_left_sleeve",
    tab: "skin",
    label: { zh: "左袖", en: "Left sleeve" },
    hint: { zh: "显示皮肤左臂外层。", en: "Show the left sleeve skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_right_sleeve",
    tab: "skin",
    label: { zh: "右袖", en: "Right sleeve" },
    hint: { zh: "显示皮肤右臂外层。", en: "Show the right sleeve skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_left_pants_leg",
    tab: "skin",
    label: { zh: "左裤腿", en: "Left pants leg" },
    hint: { zh: "显示皮肤左腿外层。", en: "Show the left pants leg skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "modelPart_right_pants_leg",
    tab: "skin",
    label: { zh: "右裤腿", en: "Right pants leg" },
    hint: { zh: "显示皮肤右腿外层。", en: "Show the right pants leg skin layer." },
    defaultValue: "true",
    control: { kind: "bool" },
  },

  // 2. 视频
  {
    key: "fov",
    tab: "video",
    label: { zh: "视场角（FOV）", en: "Field of view (FOV)" },
    hint: {
      zh: "控制游戏画面视野范围，原版默认为 70°。",
      en: "Controls camera viewing angle (default 70°).",
    },
    defaultValue: "0.0",
    control: {
      kind: "slider",
      min: 30,
      max: 110,
      step: 1,
      toUi: (raw) => Math.min(110, Math.max(30, Math.round(70 + parseFloatOr(raw, 0) * 40))),
      fromUi: (deg) => ((deg - 70) / 40).toFixed(2),
      format: (deg) => `${deg}°`,
    },
  },
  {
    key: "renderDistance",
    tab: "video",
    label: { zh: "渲染距离", en: "Render distance" },
    hint: {
      zh: "加载与显示的区块半径，越高越吃显卡和内存。",
      en: "Chunk radius rendered around the player.",
    },
    defaultValue: "12",
    control: {
      kind: "slider",
      min: 2,
      max: 32,
      step: 1,
      toUi: (raw) => Math.min(32, Math.max(2, parseIntOr(raw, 12))),
      fromUi: (v) => String(v),
      format: (v, isZh) => (isZh ? `${v} 区块` : `${v} ch`),
    },
  },
  {
    key: "simulationDistance",
    tab: "video",
    label: { zh: "模拟距离", en: "Simulation distance" },
    hint: {
      zh: "实体与红石保持活跃更新的区块半径。",
      en: "Chunk radius where entities and redstone tick.",
    },
    defaultValue: "12",
    control: {
      kind: "slider",
      min: 5,
      max: 32,
      step: 1,
      toUi: (raw) => Math.min(32, Math.max(5, parseIntOr(raw, 12))),
      fromUi: (v) => String(v),
      format: (v, isZh) => (isZh ? `${v} 区块` : `${v} ch`),
    },
  },
  {
    key: "maxFps",
    tab: "video",
    label: { zh: "最大帧率", en: "Max framerate" },
    hint: {
      zh: "限制游戏最高帧率；拉到最右（260）为无限制。",
      en: "Caps max FPS; 260 means unlimited.",
    },
    defaultValue: "120",
    control: {
      kind: "slider",
      min: 10,
      max: 260,
      step: 10,
      toUi: (raw) => Math.min(260, Math.max(10, Math.round(parseIntOr(raw, 120) / 10) * 10)),
      fromUi: (v) => String(v),
      format: (v, isZh) => (v >= 260 ? (isZh ? "无限制" : "∞") : `${v}`),
    },
  },
  {
    key: "guiScale",
    tab: "video",
    label: { zh: "界面尺寸", en: "GUI scale" },
    hint: {
      zh: "调整游戏内 HUD 与菜单缩放（0 为自动）。",
      en: "Adjusts HUD and menu scaling (0 = Auto).",
    },
    defaultValue: "0",
    control: {
      kind: "slider",
      min: 0,
      max: 4,
      step: 1,
      toUi: (raw) => Math.min(4, Math.max(0, parseIntOr(raw, 0))),
      fromUi: (v) => String(v),
      format: (v, isZh) => (v === 0 ? (isZh ? "自动" : "Auto") : `${v}x`),
    },
  },
  {
    key: "gamma",
    tab: "video",
    label: { zh: "亮度", en: "Brightness" },
    hint: { zh: "调整洞穴与夜晚画面的明暗程度。", en: "Adjusts brightness in dark areas." },
    defaultValue: "0.5",
    control: pctSlider(0.5, 0, 100, 5),
  },
  {
    key: "fullscreen",
    tab: "video",
    label: { zh: "全屏模式", en: "Fullscreen" },
    hint: { zh: "以全屏模式显示游戏画面。", en: "Run Minecraft in fullscreen mode." },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "enableVsync",
    tab: "video",
    label: { zh: "垂直同步", en: "VSync" },
    hint: {
      zh: "同步显示器刷新率以消除画面撕裂。",
      en: "Sync framerate with monitor refresh rate.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "renderClouds",
    tab: "video",
    label: { zh: "云渲染", en: "Clouds" },
    hint: { zh: "选择天空云层的渲染方式。", en: "Choose how sky clouds are rendered." },
    defaultValue: '"true"',
    control: {
      kind: "select",
      choices: [
        { value: '"false"', label: { zh: "关闭", en: "Off" } },
        { value: '"fast"', label: { zh: "流畅", en: "Fast" } },
        { value: '"true"', label: { zh: "高品质", en: "Fancy" } },
      ],
    },
  },
  {
    key: "particles",
    tab: "video",
    label: { zh: "粒子效果", en: "Particles" },
    hint: {
      zh: "减少粒子可在大型战斗或机器旁显著提帧。",
      en: "Reducing particles improves FPS near machines.",
    },
    defaultValue: "0",
    control: {
      kind: "select",
      choices: [
        { value: "0", label: { zh: "全部", en: "All" } },
        { value: "1", label: { zh: "少量", en: "Decreased" } },
        { value: "2", label: { zh: "最少", en: "Minimal" } },
      ],
    },
  },
  {
    key: "ao",
    tab: "video",
    label: { zh: "平滑光照", en: "Smooth lighting" },
    hint: {
      zh: "为方块交界处提供柔和的环境光遮蔽过渡。",
      en: "Softens lighting transitions across blocks.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "entityShadows",
    tab: "video",
    label: { zh: "实体阴影", en: "Entity shadows" },
    hint: {
      zh: "在生物与玩家脚下显示柔和圆形阴影。",
      en: "Render shadows underneath mobs and players.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "biomeBlendRadius",
    tab: "video",
    label: { zh: "生物群系过渡距离", en: "Biome blend" },
    hint: {
      zh: "控制不同群系草地与树叶颜色的平滑过渡半径。",
      en: "Smooths grass and foliage color across biomes.",
    },
    defaultValue: "2",
    control: {
      kind: "slider",
      min: 0,
      max: 7,
      step: 1,
      toUi: (raw) => Math.min(7, Math.max(0, parseIntOr(raw, 2))),
      fromUi: (v) => String(v),
      format: (v, isZh) => (v === 0 ? (isZh ? "关闭" : "Off") : `${v * 2 + 1}×${v * 2 + 1}`),
    },
  },
  {
    key: "bobView",
    tab: "video",
    label: { zh: "视角摇晃", en: "View bobbing" },
    hint: {
      zh: "行走或奔跑时画面轻微晃动；晕 3D 建议关闭。",
      en: "Subtle camera bobbing while walking.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "attackIndicator",
    tab: "video",
    label: { zh: "攻击指示器", en: "Attack indicator" },
    hint: {
      zh: "选择攻击冷却进度条的显示位置。",
      en: "Where to show the attack cooldown indicator.",
    },
    defaultValue: "1",
    control: {
      kind: "select",
      choices: [
        { value: "0", label: { zh: "关闭", en: "Off" } },
        { value: "1", label: { zh: "十字准星", en: "Crosshair" } },
        { value: "2", label: { zh: "快捷栏旁", en: "Hotbar" } },
      ],
    },
  },

  // 3. 语言
  {
    key: "lang",
    tab: "language",
    label: { zh: "游戏语言", en: "Game language" },
    hint: {
      zh: "同步所有参与实例的游戏内显示语言。",
      en: "In-game display language across synced instances.",
    },
    defaultValue: "zh_cn",
    control: {
      kind: "select",
      choices: [
        { value: "zh_cn", label: { zh: "简体中文 (zh_cn)", en: "Simplified Chinese (zh_cn)" } },
        { value: "zh_tw", label: { zh: "繁體中文 (zh_tw)", en: "Traditional Chinese (zh_tw)" } },
        { value: "zh_hk", label: { zh: "繁體中文·香港 (zh_hk)", en: "Chinese HK (zh_hk)" } },
        { value: "en_us", label: { zh: "English (US)", en: "English (US)" } },
        { value: "en_gb", label: { zh: "English (UK)", en: "English (UK)" } },
        { value: "ja_jp", label: { zh: "日本語 (ja_jp)", en: "Japanese (ja_jp)" } },
        { value: "ko_kr", label: { zh: "한국어 (ko_kr)", en: "Korean (ko_kr)" } },
        { value: "de_de", label: { zh: "Deutsch (de_de)", en: "German (de_de)" } },
        { value: "fr_fr", label: { zh: "Français (fr_fr)", en: "French (fr_fr)" } },
        { value: "ru_ru", label: { zh: "Русский (ru_ru)", en: "Russian (ru_ru)" } },
      ],
    },
  },
  {
    key: "forceUnicodeFont",
    tab: "language",
    label: { zh: "强制使用 Unicode 字体", en: "Force Unicode font" },
    hint: {
      zh: "使用更细的统一字符宽度字体渲染文字。",
      en: "Use the uniform thin Unicode glyph font.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },

  // 4. 音乐和声音
  {
    key: "soundCategory_master",
    tab: "audio",
    label: { zh: "主音量", en: "Master volume" },
    hint: { zh: "控制游戏全部声音的总体音量。", en: "Overall game volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_music",
    tab: "audio",
    label: { zh: "音乐", en: "Music" },
    hint: { zh: "背景音乐音量。", en: "Background music volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_record",
    tab: "audio",
    label: { zh: "唱片机 / 音符盒", en: "Jukebox / Note blocks" },
    hint: { zh: "唱片机与音符盒演奏音量。", en: "Jukebox and note block volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_weather",
    tab: "audio",
    label: { zh: "天气", en: "Weather" },
    hint: { zh: "下雨与雷鸣等天气音效音量。", en: "Rain and thunder volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_block",
    tab: "audio",
    label: { zh: "方块", en: "Blocks" },
    hint: { zh: "挖掘、放置方块与活塞等机械音量。", en: "Block breaking, placing, and pistons." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_hostile",
    tab: "audio",
    label: { zh: "敌对生物", en: "Hostile creatures" },
    hint: { zh: "僵尸、苦力怕、骷髅等敌对生物音量。", en: "Hostile mob sound volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_neutral",
    tab: "audio",
    label: { zh: "友好生物", en: "Friendly creatures" },
    hint: { zh: "村民、动物等友好或中立生物音量。", en: "Passive and neutral mob sound volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_player",
    tab: "audio",
    label: { zh: "玩家", en: "Players" },
    hint: {
      zh: "脚步、受伤、吃东西等玩家自身音效。",
      en: "Player footsteps, hurt, and eating sounds.",
    },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_ambient",
    tab: "audio",
    label: { zh: "环境", en: "Ambient / Environment" },
    hint: { zh: "洞穴、下界与水下环境氛围音效。", en: "Cave, Nether, and underwater ambience." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "soundCategory_voice",
    tab: "audio",
    label: { zh: "语音 / 旁白", en: "Voice / Speech" },
    hint: { zh: "游戏旁白语音音量。", en: "Narrator speech volume." },
    defaultValue: "1.0",
    control: pctSlider(1.0),
  },
  {
    key: "showSubtitles",
    tab: "audio",
    label: { zh: "显示字幕", en: "Show subtitles" },
    hint: {
      zh: "在屏幕右下角提示附近声音来源与方位。",
      en: "Display directional sound captions on screen.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "directionalAudio",
    tab: "audio",
    label: { zh: "定向音频（HRTF）", en: "Directional audio" },
    hint: {
      zh: "佩戴耳机时增强立体声空间定位感。",
      en: "Improves spatial 3D sound when using headphones.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },

  // 5. 控制
  {
    key: "mouseSensitivity",
    tab: "controls",
    label: { zh: "鼠标灵敏度", en: "Mouse sensitivity" },
    hint: {
      zh: "调整转动视角的鼠标速度（原版默认 100%）。",
      en: "Camera look speed (default 100%).",
    },
    defaultValue: "0.5",
    control: pctSlider(0.5, 0, 200, 1, 200),
  },
  {
    key: "invertYMouse",
    tab: "controls",
    label: { zh: "反转鼠标", en: "Invert mouse" },
    hint: { zh: "上下移动鼠标时反转视角方向。", en: "Invert vertical mouse look axis." },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "mouseWheelSensitivity",
    tab: "controls",
    label: { zh: "滚轮灵敏度", en: "Scroll sensitivity" },
    hint: { zh: "调整滚轮切换快捷栏或滚动列表的速度。", en: "Adjusts scroll wheel speed." },
    defaultValue: "1.0",
    control: {
      kind: "slider",
      min: 10,
      max: 500,
      step: 10,
      toUi: (raw) => Math.min(500, Math.max(10, Math.round(parseFloatOr(raw, 1.0) * 100))),
      fromUi: (ui) => (ui / 100).toFixed(2),
      format: (ui) => `${(ui / 100).toFixed(1)}x`,
    },
  },
  {
    key: "rawMouseInput",
    tab: "controls",
    label: { zh: "原始鼠标输入", en: "Raw mouse input" },
    hint: {
      zh: "直接读取鼠标硬件移动，跳过系统加速。",
      en: "Read raw hardware input bypassing OS acceleration.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "autoJump",
    tab: "controls",
    label: { zh: "自动跳跃", en: "Auto-jump" },
    hint: { zh: "靠近一格高方块时自动跳上。", en: "Automatically jump over one-block ledges." },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "toggleCrouch",
    tab: "controls",
    label: { zh: "切换潜行", en: "Toggle sneak" },
    hint: {
      zh: "开启后按一下切换潜行状态，关闭则为长按。",
      en: "Press once to toggle sneak instead of holding.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "toggleSprint",
    tab: "controls",
    label: { zh: "切换疾跑", en: "Toggle sprint" },
    hint: {
      zh: "开启后按一下切换疾跑状态，关闭则为长按。",
      en: "Press once to toggle sprint instead of holding.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "operatorItemsTab",
    tab: "controls",
    label: { zh: "管理员物品标签页", en: "Operator items tab" },
    hint: {
      zh: "创造模式背包中显示命令方块、屏障等管理员物品。",
      en: "Show command blocks and barrier in Creative inventory.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },

  // 7. 聊天
  {
    key: "chatVisibility",
    tab: "chat",
    label: { zh: "聊天可见性", en: "Chat visibility" },
    hint: {
      zh: "控制游戏左下角聊天框显示范围。",
      en: "Control which chat messages appear on screen.",
    },
    defaultValue: "0",
    control: {
      kind: "select",
      choices: [
        { value: "0", label: { zh: "全部显示", en: "Shown" } },
        { value: "1", label: { zh: "仅限命令", en: "Commands only" } },
        { value: "2", label: { zh: "隐藏", en: "Hidden" } },
      ],
    },
  },
  {
    key: "chatColors",
    tab: "chat",
    label: { zh: "聊天颜色", en: "Chat colors" },
    hint: { zh: "允许聊天消息显示服务器与玩家自定义颜色。", en: "Allow colored chat messages." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "chatLinks",
    tab: "chat",
    label: { zh: "网页链接", en: "Web links" },
    hint: { zh: "允许点击聊天栏中的网页链接。", en: "Make URLs in chat clickable." },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "chatLinksPrompt",
    tab: "chat",
    label: { zh: "打开链接前提示", en: "Prompt on links" },
    hint: {
      zh: "点击聊天中的外部链接前弹出二次确认。",
      en: "Ask before opening external links from chat.",
    },
    defaultValue: "true",
    control: { kind: "bool" },
  },
  {
    key: "chatOpacity",
    tab: "chat",
    label: { zh: "聊天文字不透明度", en: "Chat text opacity" },
    hint: { zh: "调整聊天文字自身的透明度。", en: "Opacity of chat text." },
    defaultValue: "1.0",
    control: pctSlider(1.0, 10, 100, 5),
  },
  {
    key: "textBackgroundOpacity",
    tab: "chat",
    label: { zh: "文字背景不透明度", en: "Text background opacity" },
    hint: {
      zh: "调整聊天与字幕背后的暗色底条透明度。",
      en: "Opacity of the dark backdrop behind chat and captions.",
    },
    defaultValue: "0.5",
    control: pctSlider(0.5, 0, 100, 5),
  },
  {
    key: "chatScale",
    tab: "chat",
    label: { zh: "聊天栏缩放", en: "Chat scale" },
    hint: { zh: "独立缩放聊天文字尺寸。", en: "Scale chat text size independently." },
    defaultValue: "1.0",
    control: pctSlider(1.0, 20, 100, 5),
  },
  {
    key: "chatWidth",
    tab: "chat",
    label: { zh: "聊天栏宽度", en: "Chat width" },
    hint: { zh: "调整聊天栏占用的水平宽度。", en: "Horizontal width of the chat box." },
    defaultValue: "1.0",
    control: pctSlider(1.0, 10, 100, 5),
  },
  {
    key: "chatHeightFocused",
    tab: "chat",
    label: { zh: "聚焦高度", en: "Focused height" },
    hint: { zh: "打开聊天输入时展开的最大高度。", en: "Maximum height when chat is focused." },
    defaultValue: "1.0",
    control: pctSlider(1.0, 10, 100, 5),
  },

  // 8. 无障碍
  {
    key: "narrator",
    tab: "accessibility",
    label: { zh: "旁白", en: "Narrator" },
    hint: { zh: "朗读聊天消息或系统界面按钮。", en: "Reads chat messages or system UI aloud." },
    defaultValue: "0",
    control: {
      kind: "select",
      choices: [
        { value: "0", label: { zh: "关闭", en: "Off" } },
        { value: "1", label: { zh: "全部", en: "All" } },
        { value: "2", label: { zh: "仅聊天", en: "Chat" } },
        { value: "3", label: { zh: "仅系统", en: "System" } },
      ],
    },
  },
  {
    key: "highContrast",
    tab: "accessibility",
    label: { zh: "高对比度界面", en: "High contrast" },
    hint: {
      zh: "启用 Minecraft 原版高对比度菜单样式。",
      en: "Enable built-in high-contrast UI resource pack.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "darkMojangStudiosBackground",
    tab: "accessibility",
    label: { zh: "单色加载界面", en: "Monochrome logo" },
    hint: {
      zh: "将启动红屏改为深黑底色，夜间更护眼。",
      en: "Use a dark loading screen instead of bright red.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "hideSplashTexts",
    tab: "accessibility",
    label: { zh: "隐藏标题闪烁标语", en: "Hide splash texts" },
    hint: {
      zh: "隐藏主菜单标题旁跳动的黄色标语。",
      en: "Hide animated yellow splash text on the title screen.",
    },
    defaultValue: "false",
    control: { kind: "bool" },
  },
  {
    key: "damageTiltStrength",
    tab: "accessibility",
    label: { zh: "伤害画面倾斜", en: "Damage tilt" },
    hint: {
      zh: "受伤时镜头倾斜的幅度，设为 0% 可完全关闭。",
      en: "Camera shake intensity when taking damage.",
    },
    defaultValue: "1.0",
    control: pctSlider(1.0, 0, 100, 5),
  },
  {
    key: "fovEffectScale",
    tab: "accessibility",
    label: { zh: "视场角动态效果", en: "FOV effects" },
    hint: {
      zh: "疾跑或速度药水导致的画面拉伸强度。",
      en: "Intensity of FOV stretching when sprinting or under speed.",
    },
    defaultValue: "1.0",
    control: pctSlider(1.0, 0, 100, 5),
  },
  {
    key: "screenEffectScale",
    tab: "accessibility",
    label: { zh: "畸变效果", en: "Distortion effects" },
    hint: {
      zh: "传送门与反胃药水带来的画面扭曲强度。",
      en: "Intensity of portal and nausea screen distortion.",
    },
    defaultValue: "1.0",
    control: pctSlider(1.0, 0, 100, 5),
  },
  {
    key: "darknessEffectScale",
    tab: "accessibility",
    label: { zh: "黑暗脉冲强度", en: "Darkness pulsing" },
    hint: {
      zh: "监守者黑暗效果下的明暗闪烁幅度。",
      en: "Intensity of Warden darkness effect pulsing.",
    },
    defaultValue: "1.0",
    control: pctSlider(1.0, 0, 100, 5),
  },
];

export const MC_KEYBINDS_CATALOG: readonly KeybindSpec[] = [
  {
    key: "key_key.forward",
    label: { zh: "前进", en: "Walk forwards" },
    defaultValue: "key.keyboard.w",
  },
  {
    key: "key_key.back",
    label: { zh: "后退", en: "Walk backwards" },
    defaultValue: "key.keyboard.s",
  },
  {
    key: "key_key.left",
    label: { zh: "向左移动", en: "Strafe left" },
    defaultValue: "key.keyboard.a",
  },
  {
    key: "key_key.right",
    label: { zh: "向右移动", en: "Strafe right" },
    defaultValue: "key.keyboard.d",
  },
  { key: "key_key.jump", label: { zh: "跳跃", en: "Jump" }, defaultValue: "key.keyboard.space" },
  {
    key: "key_key.sneak",
    label: { zh: "潜行", en: "Sneak" },
    defaultValue: "key.keyboard.left.shift",
  },
  {
    key: "key_key.sprint",
    label: { zh: "疾跑", en: "Sprint" },
    defaultValue: "key.keyboard.left.control",
  },
  {
    key: "key_key.attack",
    label: { zh: "攻击 / 破坏", en: "Attack / Destroy" },
    defaultValue: "key.mouse.left",
  },
  {
    key: "key_key.use",
    label: { zh: "使用物品 / 放置方块", en: "Use item / Place block" },
    defaultValue: "key.mouse.right",
  },
  {
    key: "key_key.pickItem",
    label: { zh: "选取方块", en: "Pick block" },
    defaultValue: "key.mouse.middle",
  },
  {
    key: "key_key.inventory",
    label: { zh: "打开 / 关闭物品栏", en: "Open/close inventory" },
    defaultValue: "key.keyboard.e",
  },
  {
    key: "key_key.swapOffhand",
    label: { zh: "切换副手物品", en: "Swap item with offhand" },
    defaultValue: "key.keyboard.f",
  },
  {
    key: "key_key.drop",
    label: { zh: "丢弃选中物品", en: "Drop selected item" },
    defaultValue: "key.keyboard.q",
  },
  {
    key: "key_key.chat",
    label: { zh: "打开聊天栏", en: "Open chat" },
    defaultValue: "key.keyboard.t",
  },
  {
    key: "key_key.command",
    label: { zh: "打开命令", en: "Open command" },
    defaultValue: "key.keyboard.slash",
  },
  {
    key: "key_key.playerlist",
    label: { zh: "列出玩家", en: "List players" },
    defaultValue: "key.keyboard.tab",
  },
  {
    key: "key_key.togglePerspective",
    label: { zh: "切换视角", en: "Toggle perspective" },
    defaultValue: "key.keyboard.f5",
  },
  {
    key: "key_key.screenshot",
    label: { zh: "截图", en: "Take screenshot" },
    defaultValue: "key.keyboard.f2",
  },
  {
    key: "key_key.fullscreen",
    label: { zh: "切换全屏", en: "Toggle fullscreen" },
    defaultValue: "key.keyboard.f11",
  },
  {
    key: "key_key.advancements",
    label: { zh: "进度", en: "Advancements" },
    defaultValue: "key.keyboard.l",
  },
  {
    key: "key_key.hotbar.1",
    label: { zh: "快捷栏 1", en: "Hotbar slot 1" },
    defaultValue: "key.keyboard.1",
  },
  {
    key: "key_key.hotbar.2",
    label: { zh: "快捷栏 2", en: "Hotbar slot 2" },
    defaultValue: "key.keyboard.2",
  },
  {
    key: "key_key.hotbar.3",
    label: { zh: "快捷栏 3", en: "Hotbar slot 3" },
    defaultValue: "key.keyboard.3",
  },
  {
    key: "key_key.hotbar.4",
    label: { zh: "快捷栏 4", en: "Hotbar slot 4" },
    defaultValue: "key.keyboard.4",
  },
  {
    key: "key_key.hotbar.5",
    label: { zh: "快捷栏 5", en: "Hotbar slot 5" },
    defaultValue: "key.keyboard.5",
  },
  {
    key: "key_key.hotbar.6",
    label: { zh: "快捷栏 6", en: "Hotbar slot 6" },
    defaultValue: "key.keyboard.6",
  },
  {
    key: "key_key.hotbar.7",
    label: { zh: "快捷栏 7", en: "Hotbar slot 7" },
    defaultValue: "key.keyboard.7",
  },
  {
    key: "key_key.hotbar.8",
    label: { zh: "快捷栏 8", en: "Hotbar slot 8" },
    defaultValue: "key.keyboard.8",
  },
  {
    key: "key_key.hotbar.9",
    label: { zh: "快捷栏 9", en: "Hotbar slot 9" },
    defaultValue: "key.keyboard.9",
  },
];

export const CATALOG_OPTION_KEYS = new Set<string>([
  ...MC_OPTIONS_CATALOG.map((item) => item.key),
  ...MC_KEYBINDS_CATALOG.map((item) => item.key),
]);

const SPECIAL_KEY_LABELS: Record<string, BiText> = {
  "key.keyboard.unknown": { zh: "未绑定", en: "Unbound" },
  "key.keyboard.space": { zh: "空格", en: "Space" },
  "key.keyboard.left.shift": { zh: "左 Shift", en: "Left Shift" },
  "key.keyboard.right.shift": { zh: "右 Shift", en: "Right Shift" },
  "key.keyboard.left.control": { zh: "左 Ctrl", en: "Left Ctrl" },
  "key.keyboard.right.control": { zh: "右 Ctrl", en: "Right Ctrl" },
  "key.keyboard.left.alt": { zh: "左 Alt", en: "Left Alt" },
  "key.keyboard.right.alt": { zh: "右 Alt", en: "Right Alt" },
  "key.keyboard.tab": { zh: "Tab", en: "Tab" },
  "key.keyboard.caps.lock": { zh: "Caps Lock", en: "Caps Lock" },
  "key.keyboard.enter": { zh: "Enter", en: "Enter" },
  "key.keyboard.backspace": { zh: "退格", en: "Backspace" },
  "key.keyboard.delete": { zh: "Delete", en: "Delete" },
  "key.keyboard.insert": { zh: "Insert", en: "Insert" },
  "key.keyboard.home": { zh: "Home", en: "Home" },
  "key.keyboard.end": { zh: "End", en: "End" },
  "key.keyboard.page.up": { zh: "Page Up", en: "Page Up" },
  "key.keyboard.page.down": { zh: "Page Down", en: "Page Down" },
  "key.keyboard.up": { zh: "↑", en: "Up" },
  "key.keyboard.down": { zh: "↓", en: "Down" },
  "key.keyboard.left": { zh: "←", en: "Left" },
  "key.keyboard.right": { zh: "→", en: "Right" },
  "key.keyboard.grave.accent": { zh: "` / ~", en: "`" },
  "key.keyboard.minus": { zh: "-", en: "-" },
  "key.keyboard.equal": { zh: "=", en: "=" },
  "key.keyboard.left.bracket": { zh: "[", en: "[" },
  "key.keyboard.right.bracket": { zh: "]", en: "]" },
  "key.keyboard.backslash": { zh: "\\", en: "\\" },
  "key.keyboard.semicolon": { zh: ";", en: ";" },
  "key.keyboard.apostrophe": { zh: "'", en: "'" },
  "key.keyboard.comma": { zh: ",", en: "," },
  "key.keyboard.period": { zh: ".", en: "." },
  "key.keyboard.slash": { zh: "/", en: "/" },
  "key.mouse.left": { zh: "鼠标左键", en: "Left Click" },
  "key.mouse.right": { zh: "鼠标右键", en: "Right Click" },
  "key.mouse.middle": { zh: "鼠标中键", en: "Middle Click" },
  "key.mouse.4": { zh: "鼠标侧键 4", en: "Mouse 4" },
  "key.mouse.5": { zh: "鼠标侧键 5", en: "Mouse 5" },
};

/** 将 Minecraft LWJGL 键名翻译成玩家一眼看懂的键帽文本 */
export function formatMcKey(raw: string | undefined, isZh: boolean): string {
  if (!raw || raw === "key.keyboard.unknown") {
    return isZh ? "未绑定" : "Unbound";
  }
  const hit = SPECIAL_KEY_LABELS[raw];
  if (hit) return isZh ? hit.zh : hit.en;
  if (raw.startsWith("key.keyboard.keypad.")) {
    const tail = raw.slice("key.keyboard.keypad.".length).toUpperCase();
    return isZh ? `小键盘 ${tail}` : `Num ${tail}`;
  }
  if (raw.startsWith("key.keyboard.")) {
    return raw.slice("key.keyboard.".length).toUpperCase();
  }
  if (raw.startsWith("key.mouse.")) {
    const num = raw.slice("key.mouse.".length);
    return isZh ? `鼠标键 ${num}` : `Mouse ${num}`;
  }
  return raw;
}

const DOM_CODE_TO_MC: Record<string, string> = {
  Space: "key.keyboard.space",
  ShiftLeft: "key.keyboard.left.shift",
  ShiftRight: "key.keyboard.right.shift",
  ControlLeft: "key.keyboard.left.control",
  ControlRight: "key.keyboard.right.control",
  AltLeft: "key.keyboard.left.alt",
  AltRight: "key.keyboard.right.alt",
  Tab: "key.keyboard.tab",
  CapsLock: "key.keyboard.caps.lock",
  Enter: "key.keyboard.enter",
  Backspace: "key.keyboard.backspace",
  Delete: "key.keyboard.delete",
  Insert: "key.keyboard.insert",
  Home: "key.keyboard.home",
  End: "key.keyboard.end",
  PageUp: "key.keyboard.page.up",
  PageDown: "key.keyboard.page.down",
  ArrowUp: "key.keyboard.up",
  ArrowDown: "key.keyboard.down",
  ArrowLeft: "key.keyboard.left",
  ArrowRight: "key.keyboard.right",
  Backquote: "key.keyboard.grave.accent",
  Minus: "key.keyboard.minus",
  Equal: "key.keyboard.equal",
  BracketLeft: "key.keyboard.left.bracket",
  BracketRight: "key.keyboard.right.bracket",
  Backslash: "key.keyboard.backslash",
  Semicolon: "key.keyboard.semicolon",
  Quote: "key.keyboard.apostrophe",
  Comma: "key.keyboard.comma",
  Period: "key.keyboard.period",
  Slash: "key.keyboard.slash",
};

/** 将浏览器 KeyboardEvent.code 转换为 Minecraft LWJGL 键名 */
export function domKeyCodeToMcKey(code: string): string | null {
  if (code in DOM_CODE_TO_MC) return DOM_CODE_TO_MC[code] ?? null;
  if (/^Key[A-Z]$/.test(code)) {
    return `key.keyboard.${code.slice(3).toLowerCase()}`;
  }
  if (/^Digit[0-9]$/.test(code)) {
    return `key.keyboard.${code.slice(5)}`;
  }
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) {
    return `key.keyboard.${code.toLowerCase()}`;
  }
  if (/^Numpad[0-9]$/.test(code)) {
    return `key.keyboard.keypad.${code.slice(6)}`;
  }
  return null;
}

/** 将鼠标按钮编号转换为 Minecraft LWJGL 键名 */
export function domMouseButtonToMcKey(button: number): string | null {
  if (button === 0) return "key.mouse.left";
  if (button === 1) return "key.mouse.middle";
  if (button === 2) return "key.mouse.right";
  if (button === 3) return "key.mouse.4";
  if (button === 4) return "key.mouse.5";
  return null;
}
