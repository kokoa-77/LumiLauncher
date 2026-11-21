// 整个界面的浏览器预览（只在开发时用）：假装有后端，喂假数据，不用编译 Rust、不开启动器就能点来点去。
// `pnpm --dir app dev` 后浏览器打开 http://localhost:1420/preview.html 。正式包不包含它（vite 只打包 index.html）。
// 后端做的事（下载、启动游戏）这里都不会真的发生。
import type { AccountsState, AccountView, UpdateMode, UpdateOffer } from "@/lib/bindings";
import cobblemonIcon from "./fixtures/icon-cobblemon.png";
import fomIcon from "./fixtures/icon-fom.png";
import optimizationIcon from "./fixtures/icon-optimization.png";
// 原创的抽象人形（透明底的竖图），代替「自己选的立绘」看完整显示的样子
import standinArt from "./fixtures/standin-art.png";
import worldShot from "./fixtures/world-shot.jpg";

const now = Date.now();
const HOUR = 3_600_000;

type PreviewInstance = {
  id: string;
  name: string;
  gameVersion: string;
  loader: string | null;
  pack: string | null;
  createdAt: number;
  lastPlayed: number | null;
  group: string | null;
  pinnedAt: number | null;
  /** 挂上的别家版本是哪家的 */
  linked?: string | null;
};

const instances: PreviewInstance[] = [
  {
    id: "验收测试",
    name: "验收测试",
    gameVersion: "1.21.1",
    loader: "NeoForge 21.1.265",
    pack: null,
    createdAt: now - 72 * HOUR,
    lastPlayed: now - 2 * HOUR,
    group: null,
    pinnedAt: now - 10 * HOUR,
  },
  {
    id: "Cobblemon Official Modpack [NeoForge]",
    name: "Cobblemon Official Modpack [NeoForge]",
    gameVersion: "1.21.1",
    loader: "NeoForge 21.1.209",
    pack: "Cobblemon Official Modpack [NeoForge]",
    createdAt: now - 60 * HOUR,
    lastPlayed: now - 26 * HOUR,
    group: "整合包",
    pinnedAt: null,
  },
  {
    id: "命运齿轮FOM机械动力航空学【Fate's Machinery】",
    name: "命运齿轮FOM机械动力航空学【Fate's Machinery】",
    gameVersion: "1.21.1",
    loader: "NeoForge 21.1.215",
    pack: "命运齿轮FOM机械动力航空学【Fate's Machinery】",
    createdAt: now - 50 * HOUR,
    lastPlayed: now - 70 * HOUR,
    group: "整合包",
    pinnedAt: null,
  },
  {
    id: "Optimization",
    name: "Optimization",
    gameVersion: "1.21.1",
    loader: "NeoForge 21.1.248",
    pack: "Optimization",
    createdAt: now - 90 * HOUR,
    lastPlayed: now - 160 * HOUR,
    group: "测试",
    pinnedAt: null,
  },
  {
    id: "1.21.1",
    name: "1.21.1",
    gameVersion: "1.21.1",
    loader: null,
    pack: null,
    createdAt: now - 95 * HOUR,
    lastPlayed: null,
    group: null,
    pinnedAt: null,
  },
  // 挂上的 PCL 版本
  {
    id: "1.21.1-NeoForge_21.1.244",
    name: "1.21.1-NeoForge_21.1.244",
    gameVersion: "1.21.1",
    loader: "NeoForge 21.1.244",
    pack: null,
    createdAt: now - 20 * HOUR,
    lastPlayed: null,
    group: "PCL2",
    pinnedAt: null,
    linked: "pcl",
  },
  {
    id: "1.20.1-Fabric 0.15.3",
    name: "1.20.1-Fabric 0.15.3",
    gameVersion: "1.20.1",
    loader: "Fabric 0.15.3",
    pack: null,
    createdAt: now - 20 * HOUR,
    lastPlayed: null,
    group: "PCL2",
    pinnedAt: null,
    linked: "pcl",
  },
];

// 挂上的别家目录、找到的目录、别家的实例
const linkedRoots = [
  {
    id: "root-pcl2",
    path: String.raw`D:\PCL2\.minecraft`,
    name: "PCL2",
    launcher: "pcl",
    versions: 5,
    missing: false,
    hidden: 1,
  },
];
const foundRoots = [
  {
    id: "root-farm",
    path: String.raw`D:\MC意外农场模组\.minecraft`,
    name: "MC意外农场模组",
    launcher: "pcl",
    versions: 1,
    missing: false,
    hidden: 0,
  },
];
const foreignInstances = [
  {
    id: "mr-1",
    launcher: "modrinthApp",
    name: "NeoForge 26.3",
    gameVersion: "26.3",
    loader: "NeoForge 26.3.0.103",
    supported: true,
  },
  {
    id: "prism-1",
    launcher: "prism",
    name: "Fabulously Optimized",
    gameVersion: "1.21.1",
    loader: "Fabric 0.16.10",
    supported: false,
  },
];
/** 设置里「添加文件夹」选到的 Prism 实例 */
const pickedForeign = {
  id: "prism-2",
  launcher: "prism",
  name: "验收 Prism 实例",
  gameVersion: "1.21.1",
  loader: "NeoForge 21.1.265",
  supported: true,
};

const world = (
  folder: string,
  name: string,
  day: number,
  hoursAgo: number,
  image: string,
  kind: "screenshot" | "icon" | "auto" | "custom",
  fit: "cover" | "contain" = "cover",
) => ({
  folder,
  name,
  lastPlayed: now - hoursAgo * HOUR,
  day,
  gameMode: "survival",
  hardcore: false,
  version: "1.21.1",
  playSeconds: day * 1100,
  imageUrl: image,
  imageKind: kind,
  imageFit: fit,
  seed: "-4172144997902289642",
  note: null as { kind: "todo" | "text"; text: string } | null,
});

// 世界笔记：实例 ID + 存档文件夹 → 笔记
type FakeNotes = {
  places: { name: string; x: number; y: number; z: number; dimension: string }[];
  todos: { text: string; done: boolean }[];
  text: string;
};
const worldNotes = new Map<string, FakeNotes>([
  [
    "验收测试/新的世界",
    {
      places: [
        { name: "家", x: 120, y: 64, z: -340, dimension: "overworld" },
        { name: "下界传送门", x: 15, y: 70, z: -42, dimension: "nether" },
      ],
      todos: [
        { text: "砍够一组橡木", done: true },
        { text: "去找要塞", done: false },
      ],
      text: "村庄在出生点东边，铁匠铺有钻石马铠",
    },
  ],
]);
const noteLine = (notes: FakeNotes | undefined) => {
  const todo = notes?.todos.find((t) => !t.done);
  if (todo) return { kind: "todo" as const, text: todo.text };
  const line = notes?.text.split("\n").find((l) => l.trim());
  return line ? { kind: "text" as const, text: line.trim() } : null;
};

// 世界备份：实例 ID + 存档文件夹 → 备份（新的在前）
type FakeBackup = { id: string; at: number; reason: string; size: number };
const worldBackups = new Map<string, FakeBackup[]>([
  [
    "验收测试/新的世界",
    [
      { id: "3", at: Date.now() - 2 * 3600_000, reason: "afterPlay", size: 48_200_000 },
      { id: "2", at: Date.now() - 26 * 3600_000, reason: "afterPlay", size: 47_900_000 },
      { id: "1", at: Date.now() - 5 * 86400_000, reason: "manual", size: 41_000_000 },
    ],
  ],
]);

// 自己选的背景图（换背景图、完整显示 / 铺满、恢复默认都只改这里）：实例 ID + 存档文件夹 → 图
const backgrounds = new Map<string, { url: string; fit: "cover" | "contain" }>([
  ["Cobblemon Official Modpack [NeoForge]/Tutorial World v4", { url: standinArt, fit: "contain" }],
]);
const backgroundKey = (args: Record<string, unknown>) =>
  `${String(args.instanceId)}/${String(args.folder)}`;
/** 选过图的世界换成那张图 */
function withBackground<T extends { imageUrl: string | null; imageKind: string | null }>(
  instanceId: string,
  w: T & { folder: string },
): T {
  const custom = backgrounds.get(`${instanceId}/${w.folder}`);
  const note = noteLine(worldNotes.get(`${instanceId}/${w.folder}`));
  const withNote = { ...w, note };
  return custom
    ? { ...withNote, imageUrl: custom.url, imageKind: "custom", imageFit: custom.fit }
    : withNote;
}

const lockedAutoCovers = new Map<string, string>();
const lockedContent = new Set<string>();
const previewRecentServers = [
  { instanceId: "验收测试", quickPlay: true, name: "微光生存服", address: "play.lumi.example" },
  {
    instanceId: "Cobblemon Official Modpack [NeoForge]",
    quickPlay: true,
    name: "Cobblemon 官方服",
    address: "mc.cobblemon.example",
  },
];

const recent = [
  {
    instanceId: "验收测试",
    quickPlay: true,
    world: world("新的世界", "新的世界", 3, 2, worldShot, "screenshot"),
  },
  {
    instanceId: "Cobblemon Official Modpack [NeoForge]",
    quickPlay: true,
    world: world("Tutorial World v4", "Cobbled Farms", 42, 26, cobblemonIcon, "icon"),
  },
  {
    instanceId: "命运齿轮FOM机械动力航空学【Fate's Machinery】",
    quickPlay: true,
    world: world("新的世界", "齿轮之城", 7, 70, worldShot, "auto"),
  },
  {
    instanceId: "Optimization",
    quickPlay: true,
    world: world("New World", "New World", 301, 160, optimizationIcon, "icon"),
  },
];

// 资源中心的假数据：三十来个项目，能翻页、能看大图卡片和合并两个平台
const sampleProjects: [string, string, string, string[], "client" | "both" | "either"][] = [
  ["Sodium", "现代的渲染引擎，大幅提升帧率，减少卡顿。", "jellysquid3", ["optimization"], "client"],
  [
    "Iris Shaders",
    "兼容 OptiFine 光影包的光影加载器。",
    "coderbot",
    ["decoration", "optimization"],
    "client",
  ],
  [
    "Create",
    "机械动力：齿轮、传送带和会动的机器。",
    "simibubi",
    ["technology", "decoration"],
    "both",
  ],
  ["Jade", "看着方块就显示它是什么、属于哪个模组。", "Snownee", ["utility"], "client"],
  ["Waystones", "传送石碑，在世界各处快速移动。", "BlayTheNinth", ["transportation"], "both"],
  ["AppleSkin", "在饱食度条上显示食物能回复多少。", "squeek502", ["food", "utility"], "client"],
  [
    "Farmer's Delight",
    "农夫乐事：种田、做菜、煮汤。",
    "vectorwing",
    ["food", "decoration"],
    "both",
  ],
  ["Xaero's Minimap", "小地图，带路点和洞穴模式。", "xaero96", ["utility"], "client"],
  ["Entity Culling", "看不见的实体不渲染，省帧率。", "tr7zw", ["optimization"], "client"],
  ["Ferrite Core", "降低内存占用。", "malte0811", ["optimization"], "either"],
  ["Supplementaries", "上百种小装饰和实用小东西。", "MehVahdJukaar", ["decoration"], "both"],
  ["Comforts", "睡袋和吊床。", "TheIllusiveC4", ["decoration"], "both"],
];
const sampleIcons = [optimizationIcon, cobblemonIcon, fomIcon];
const PAGE = 20;
const TOTAL = 36;

/** 假的中文名：一部分项目有，看得到「中文名 + 灰色英文名」 */
const previewZh: Record<string, string> = {
  Sodium: "钠",
  Create: "机械动力",
  Waystones: "传送石碑",
  "Farmer's Delight": "农夫乐事",
  "Xaero's Minimap": "Xaero 的小地图",
  Supplementaries: "锦致装饰",
};
/** 假的中文名表（中文搜索用） */
const previewNames = [
  { zh: "机械动力", en: "Create", modrinth: "modrinth-mod-2", curseforge: null },
  {
    zh: "机械动力：蒸汽与铁路",
    en: "Create: Steam 'n' Rails",
    modrinth: "rails",
    curseforge: null,
  },
  { zh: "钠", en: "Sodium", modrinth: "modrinth-mod-0", curseforge: null },
  { zh: "传送石碑", en: "Waystones", modrinth: "modrinth-mod-4", curseforge: null },
  { zh: "农夫乐事", en: "Farmer's Delight", modrinth: "modrinth-mod-6", curseforge: null },
];

const fakeHit = (i: number, source: "modrinth" | "curseForge", kind: string) => {
  const [title, description, author, cats, side] =
    sampleProjects[i % sampleProjects.length] ?? sampleProjects[0]!;
  const round = Math.floor(i / sampleProjects.length);
  const name = round === 0 ? title : `${title} ${round + 1}`;
  return {
    projectId: `${source}-${kind}-${i}`,
    slug: name.toLowerCase().replace(/[^a-z0-9]+/g, "-"),
    title: name,
    description,
    author,
    iconUrl: i % 4 === 3 ? null : sampleIcons[i % sampleIcons.length],
    downloads: Math.round(2.5e8 / (i + 1)),
    // 每三个里有一个分类很多，卡片上会收成「+N」
    displayCategories:
      i % 3 === 0
        ? [...new Set([...cats, "utility", "library", "decoration", "technology", "magic"])]
        : cats,
    source,
    side: source === "modrinth" ? side : null,
    pageUrl: `https://modrinth.com/mod/${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
    galleryUrl: i % 3 === 2 ? null : worldShot,
    authorUrl: `https://modrinth.com/user/${author}`,
    updated: new Date(now - (i + 1) * 5 * HOUR).toISOString(),
    // 发布时间和更新时间顺序不一样：切「最新发布」看得出变化
    published: new Date(now - ((i * 7) % 23) * 30 * 24 * HOUR).toISOString(),
    titleZh: round === 0 ? (previewZh[title] ?? null) : null,
    twinId: null,
  };
};

const fakePage = (source: "modrinth" | "curseForge", args: Record<string, unknown>) => {
  const offset = Number(args.offset ?? 0);
  const kind = String(args.kind ?? "modpack");
  const query = String(args.query ?? "")
    .trim()
    .toLowerCase();
  // CurseForge 这边只有一半，名字和 Modrinth 有重叠：看得到「合成一条」
  const total = source === "curseForge" ? TOTAL / 2 : TOTAL;
  const all = Array.from({ length: total }, (_, i) =>
    fakeHit(source === "curseForge" ? i * 2 : i, source, kind),
  );
  const category = args.category ? String(args.category) : null;
  // 分类：假数据只有几种分类，其余的按编号挑一部分，看得出「筛过了」
  const inCategory = (h: ReturnType<typeof fakeHit>, i: number) =>
    !category || h.displayCategories.includes(category) || i % 3 === 0;
  const matched = (query ? all.filter((h) => h.title.toLowerCase().includes(query)) : all).filter(
    inCategory,
  );
  const sort = String(args.sort ?? "relevance");
  const time = (iso: string) => Date.parse(iso);
  const sorted =
    sort === "updated"
      ? [...matched].sort((a, b) => time(b.updated) - time(a.updated))
      : sort === "newest"
        ? [...matched].sort((a, b) => time(b.published) - time(a.published))
        : sort === "downloads"
          ? [...matched].sort((a, b) => b.downloads - a.downloads)
          : matched;
  return { hits: sorted.slice(offset, offset + PAGE), total: sorted.length, offset };
};

// 服务器：一半要先装服务器自己的整合包
const serverPage = (args: Record<string, unknown>) => {
  const page = fakePage("modrinth", { ...args, kind: "server" });
  return {
    ...page,
    hits: page.hits.map((hit, i) => ({
      ...hit,
      side: null,
      server: {
        address: `${hit.slug}.example.net`,
        playersOnline: i % 5 === 4 ? null : 1200 - i * 37,
        playersMax: i % 5 === 4 ? null : 5000,
        gameVersions: ["1.21.1", "1.21", "1.20.1"],
        modpackVersion: i % 2 === 0 ? `pack-${i}` : null,
        modpackProject: i % 2 === 0 ? hit.projectId : null,
      },
    })),
  };
};

/** 实例多人游戏列表里已有的服务器 */
let joinedServers: { name: string; address: string }[] = [
  { name: "Sodium", address: "sodium.example.net" },
  { name: "家里的服务器", address: "192.168.1.20:25565" },
  { name: "远方的服务器", address: "far.example.org" },
];

/** 服务器状态：地址里带 192.168 的连不上，far 开头的延迟高，其余按地址算一个人数 */
function fakeServerStatus(address: string) {
  if (address.includes("192.168")) return { kind: "offline" as const, reason: "refused" as const };
  const seed = Array.from(address).reduce((n, c) => n + c.charCodeAt(0), 0);
  return {
    kind: "online" as const,
    latencyMs: address.startsWith("far") ? 320 : 30 + (seed % 90),
    playersOnline: seed % 300,
    playersMax: 500,
    version: "Paper 1.21.1",
    motd: "欢迎来玩\n生存 · 空岛 · 小游戏",
    favicon: null,
  };
}

const screenshots = Array.from({ length: 9 }, (_, i) => ({
  url: worldShot,
  name: `2026-11-${String(12 - i).padStart(2, "0")}_20.1${i}.0${i}.png`,
  takenAt: now - (i * 9 + 1) * HOUR,
}));

const mods = [
  "Sodium",
  "Iris Shaders",
  "Jade",
  "Waystones",
  "Architectury API",
  "Balm",
  "AppleSkin",
];

let favoriteItems: { kind: string; hit: unknown; addedAt: number }[] = [];

// 启动记录：一次崩溃（有崩溃报告）、一次正常、一次被结束、一次没启动起来
const recordMods = mods.map((title, i) => ({
  title,
  version: `${1 + i}.0.${i}`,
  fileName: `${title.toLowerCase().replace(/ /g, "-")}.jar`,
  enabled: i !== 2,
  source: i % 3 === 2 ? "file" : "modrinth",
}));
const record = (
  id: string,
  minutesAgo: number,
  minutes: number,
  outcome: string,
  extra: Record<string, unknown> = {},
) => ({
  id,
  startedAt: now - minutesAgo * 60_000,
  endedAt: now - (minutesAgo - minutes) * 60_000,
  gameVersion: "1.21.1",
  loader: "NeoForge 21.1.265",
  javaPath: String.raw`C:\Games\Lumi\.minecraft\.mcl-java\java-runtime-delta\bin\javaw.exe`,
  javaVersion: "21.0.7",
  memoryMb: 6144,
  quickPlay: { kind: "world", folder: "新的世界" },
  mods: recordMods,
  outcome,
  exitCode: 0,
  reachedWorld: true,
  error: null,
  hasCrashReport: false,
  errorCode: null,
  logLines: 3,
  omittedLines: 0,
  ...extra,
});
const records = [
  record("1763000000000-aa11bb", 25, 1, "crashed", {
    exitCode: -1,
    reachedWorld: false,
    hasCrashReport: true,
  }),
  record("1762990000000-cc22dd", 26 * 60, 42, "exited"),
  record("1762980000000-ee33ff", 3 * 24 * 60, 95, "stopped", { quickPlay: null }),
  record("1762970000000-gg44hh", 4 * 24 * 60, 0, "failed", {
    javaVersion: null,
    memoryMb: null,
    reachedWorld: false,
    logLines: 0,
    mods: [],
    error: "下载 Java 失败：网络连接超时，请检查网络后重试",
  }),
];
const recordLog = (id: string) =>
  id.startsWith("1763000000000")
    ? [
        {
          level: "info",
          thread: "main",
          message: "ModLauncher running: args [--username, Kokoa77]",
        },
        { level: "info", thread: "Render thread", message: "Setting user: Kokoa77" },
        {
          level: "warn",
          thread: "Render thread",
          message: "Missing sound for event: crashtest:boom",
        },
        ...Array.from({ length: 60 }, (_, i) => ({
          level: i % 7 === 0 ? "warn" : "info",
          thread: "modloading-worker-0",
          message: `Loading mod ${mods[i % mods.length]} (${i})`,
        })),
        {
          level: "error",
          thread: "Render thread",
          message:
            "Crash test mod says boom\njava.lang.RuntimeException: boom\n\tat crashtest.CrashTest.onTick(CrashTest.java:12)",
        },
        {
          level: "fatal",
          thread: "Render thread",
          message:
            "#@!@# Game crashed! Crash report saved to: #@!@# crash-reports\\crash-2026-11-12_15.00.00-client.txt",
        },
      ]
    : [
        { level: "info", thread: "Render thread", message: "Setting user: Kokoa77" },
        { level: "info", thread: "Render thread", message: "Sound engine started" },
        { level: "info", thread: "Server thread", message: "Stopping server" },
      ];

// 核对界面用：preview.html?theme=dark&fx=lite 直接切深浅色和画面效果（截图脚本靠它）
const query = new URLSearchParams(location.search);
let settings = {
  schemaVersion: 2,
  appearance: {
    theme: query.get("theme") ?? "system",
    reduceMotion: query.get("motion") === "reduce",
    autoCover: true,
    accentFollowWorld: true,
    accentHue: 245,
    accentChroma: 0.15,
    effects: query.get("fx") ?? "auto",
  },
  download: {
    source: "auto",
    metaSource: "auto",
    modrinthSource: "auto",
    curseforgeSource: "auto",
    dohEnabled: true,
    maxConcurrency: 32,
    moveClaimedFiles: false,
    preferredPlatform: "modrinth",
    autoDependencies: true,
    claimFolder: null,
    proxy: { mode: "system", scheme: "http", host: "", port: 7890 },
  },
  game: {
    playerName: "Kokoa77",
    memoryAuto: true,
    maxMemoryMb: 4096,
    directory: null,
    gcPreset: "auto",
    jvmArgs: "",
    gameArgs: "",
    highPerformanceGpu: true,
    releaseWebviewWhilePlaying: false,
    defaultLanguage: "followLauncher",
    afterLaunch: "keep",
    refocusOnExit: true,
    window: { mode: "default", width: 1280, height: 720 },
    envVars: [] as { name: string; value: string }[],
    hooks: { preLaunch: "", postExit: "" },
    customWindowTitle: "",
    discordRpc: false,
  },
  worlds: { autoBackup: false, keepBackups: 3, backupDir: null as string | null },
  sync: {
    enabled: false,
    options: true,
    optionsCategories: {
      keybinds: true,
      audio: true,
      video: false,
      accessibility: true,
    },
    servers: true,
    resourcePacks: false,
    shaderPacks: false,
    commandHistory: false,
    hotbar: false,
  },
  autoCheckUpdates: true,
};
let previewCacheBytes = 148_500_000;
let previewCacheFiles = 342;
let previewUnusedVersions = [
  { id: "1.19.4", bytes: 24_600_000 },
  { id: "1.20.4-fabric-0.15.11", bytes: 4_100_000 },
];

let previewSharedOptions: Record<string, string> = {
  fov: "0.25",
  renderDistance: "16",
  maxFps: "140",
  soundCategory_master: "0.80",
  lang: "zh_cn",
  "key_key.sprint": "key.keyboard.left.control",
};
let previewSharedServers = [
  { name: "微光生存服", address: "play.lumi.example" },
  { name: "Cobblemon 官方服", address: "mc.cobblemon.example" },
];
let previewSharedPacks: Record<string, { name: string; isDir: boolean; bytes: number }[]> = {
  resourcePacks: [{ name: "Faithful-32x.zip", isDir: false, bytes: 18_400_000 }],
  shaderPacks: [{ name: "ComplementaryReimagined_r5.4.zip", isDir: false, bytes: 320_000 }],
};
let previewSharedCommands: string[] = ["/gamemode creative", "/time set day", "/weather clear"];
let previewSharedHotbarBytes: number | null = 4_096;

// 实例自己的设置、导出、复制、删除
type Overrides = {
  memory: { auto: boolean; maxMemoryMb: number } | null;
  javaId: string | null;
  launch: { gcPreset: string; jvmArgs: string; gameArgs: string } | null;
  window: { mode: string; width: number; height: number } | null;
  envVars: { name: string; value: string }[] | null;
  syncDisabled: boolean;
  hooks: { preLaunch: string; postExit: string } | null;
  customWindowTitle: string | null;
};
const instanceOverrides = new Map<string, Overrides>([
  [
    "命运齿轮FOM机械动力航空学【Fate's Machinery】",
    {
      memory: { auto: false, maxMemoryMb: 8192 },
      javaId: null,
      launch: null,
      window: null,
      envVars: null,
      syncDisabled: false,
      hooks: null,
      customWindowTitle: null,
    },
  ],
]);
const noOverrides: Overrides = {
  memory: null,
  javaId: null,
  launch: null,
  window: null,
  envVars: null,
  syncDisabled: false,
  hooks: null,
  customWindowTitle: null,
};

/** 假任务：两秒里分十步跑完，结束时调 `onDone`（导出、复制用） */
function fakeJob(key: string, title: string, onDone?: () => void): string {
  const id = `job-${key}-${Date.now()}`;
  const startedAt = Date.now();
  const total = 180_000_000;
  const running = (state: string) => state === "running";
  const job = (state: string, done: number) => ({
    id,
    key,
    title,
    state,
    progress: { done, total, unit: "bytes", message: "打包" },
    error: null,
    startedAt,
    finishedAt: running(state) ? null : Date.now(),
    source: key.startsWith("content:") ? "modrinth" : "local",
    instanceId:
      key.startsWith("instance-create:import:") && running(state)
        ? null
        : (key.split(":")[1] ?? null),
    iconUrl: null,
    steps: [{ label: "打包", at: startedAt }],
    speed: running(state) && done > 0 ? 18_000_000 : null,
    eta: running(state) && done > 0 ? (total - done) / 18_000_000 : null,
    retryable: key.startsWith("content:"),
    downloadSource: null,
    fallbacks: 0,
  });
  for (let i = 0; i < 10; i += 1) {
    setTimeout(() => emitPreview("job-updated", job("running", (total * i) / 10)), 50 + i * 200);
  }
  setTimeout(() => {
    onDone?.();
    emitPreview("job-updated", job("succeeded", total));
  }, 2100);
  return id;
}

/** 假启动：准备 1.5 秒 → 启动中 → 加载 → 主菜单 → 正在连接服务器 → 进了世界；点「结束」就退出 */
const fakeRuns = new Map<string, number[]>();
function fakeLaunch(instanceId: string) {
  const startedAt = Date.now();
  const session = (state: Record<string, unknown>) => ({
    instanceId,
    state,
    startedAt,
    error: null,
    recordId: null,
  });
  const jobId = `job-launch-${startedAt}`;
  const job = (state: string) => ({
    id: jobId,
    key: `launch:${instanceId}`,
    title: `准备启动 ${instanceId}`,
    state,
    progress: { done: 0, total: null, unit: "bytes", message: "补齐文件" },
    error: null,
    startedAt,
    finishedAt: state === "running" ? null : Date.now(),
    source: "game",
    instanceId,
    iconUrl: null,
    steps: [],
    speed: null,
    eta: null,
    retryable: false,
    downloadSource: "official",
    fallbacks: 0,
  });
  emitPreview("job-updated", job("running"));
  emitPreview("session-updated", session({ kind: "starting" }));
  const steps: [number, () => void][] = [
    [1500, () => emitPreview("job-updated", job("succeeded"))],
    [3000, () => emitPreview("session-updated", session({ kind: "loading" }))],
    [4500, () => emitPreview("session-updated", session({ kind: "mainMenu" }))],
    [5500, () => emitPreview("session-updated", session({ kind: "connecting" }))],
    [9000, () => emitPreview("session-updated", session({ kind: "inWorld" }))],
  ];
  fakeRuns.set(
    instanceId,
    steps.map(([ms, step]) => window.setTimeout(step, ms)),
  );
  return { jobId, startedAt, session, job };
}
const fakeSessions = new Map<string, ReturnType<typeof fakeLaunch>>();

function exportPreview(id: string) {
  const instance = instances.find((i) => i.id === id);
  return {
    name: instance?.name ?? id,
    gameVersion: instance?.gameVersion ?? "1.21.1",
    loader: instance?.loader ?? null,
    linked: 64,
    bundled: ["appleskin-local-3.0.9.jar", "Jade-15.10.3.jar", "我的材质包"],
    bundledBytes: 3_400_000,
    disabled: 2,
    entries: [
      { name: "config", folder: true, size: 4_800_000, defaultOn: true },
      { name: "defaultconfigs", folder: true, size: 52_000, defaultOn: true },
      { name: "journeymap", folder: true, size: 286_000_000, defaultOn: false },
      { name: "kubejs", folder: true, size: 1_200_000, defaultOn: true },
      { name: "options.txt", folder: false, size: 4_050, defaultOn: true },
      {
        name: "shaderpacks/ComplementaryReimagined_r5.4.zip.txt",
        folder: false,
        size: 900,
        defaultOn: true,
      },
    ],
    worlds:
      id === "验收测试"
        ? [
            { folder: "新的世界", name: "新的世界", size: 38_000_000 },
            { folder: "Voidblock", name: "Voidblock 空岛", size: 12_500_000 },
          ]
        : [],
  };
}

// 内存与 Java：控制台里 `__previewMemory(3000)` 把「可用」改成 3000 MB，看内存不够的提示
const TOTAL_MB = 32_563;
let availableMb = 9_400;
/** 每个实例的模组数（「命运齿轮」136 个，可用 4.5 GB 时自动模式想要 5.5 GB、给不够） */
const modCounts: Record<string, number> = {
  验收测试: 12,
  "Cobblemon Official Modpack [NeoForge]": 73,
  "命运齿轮FOM机械动力航空学【Fate's Machinery】": 136,
  Optimization: 40,
};
function memoryCheck(instanceId: string) {
  const mods = modCounts[instanceId] ?? 0;
  const auto = settings.game.memoryAuto;
  const half = TOTAL_MB / 2;
  const spare = Math.max(256, Math.floor((availableMb - 1024) / 256) * 256);
  const wanted = auto
    ? Math.floor(Math.min(Math.min(2560 + 24 * mods, 16384), half) / 256) * 256
    : settings.game.maxMemoryMb;
  const autoGive = Math.max(Math.min(wanted, spare), 2048);
  return {
    auto,
    mods,
    wantedMb: wanted,
    fromPack: false,
    spareMb: spare,
    totalMb: TOTAL_MB,
    availableMb,
    enough: wanted <= availableMb - 1024,
    fallbackMb: auto ? autoGive : Math.min(Math.max(spare, 2048), wanted),
  };
}
type PreviewJava = {
  id: string;
  major: number;
  version: string;
  vendor: string | null;
  is64bit: boolean;
  origin: "downloaded" | "manual" | "official" | "system";
  location: string;
};
let javas: PreviewJava[] = [
  {
    id: "a1b2c3d4e5f6",
    major: 21,
    version: "21.0.7",
    vendor: "Microsoft",
    is64bit: true,
    origin: "downloaded",
    location: String.raw`C:\Games\Lumi\.minecraft\.mcl-java\java-runtime-delta`,
  },
  {
    id: "b2c3d4e5f6a1",
    major: 21,
    version: "21.0.3",
    vendor: "Eclipse Adoptium",
    is64bit: true,
    origin: "system",
    location: String.raw`C:\Program Files\Eclipse Adoptium\jdk-21.0.3.9-hotspot`,
  },
  {
    id: "c3d4e5f6a1b2",
    major: 17,
    version: "17.0.15",
    vendor: "Microsoft",
    is64bit: true,
    origin: "official",
    location: String.raw`C:\Users\Player\AppData\Roaming\.minecraft\runtime\java-runtime-gamma`,
  },
  {
    id: "d4e5f6a1b2c3",
    major: 8,
    version: "1.8.0_451",
    vendor: "Oracle Corporation",
    is64bit: false,
    origin: "manual",
    location: String.raw`D:\Java\jre1.8.0_451`,
  },
];
const javaDefaults = new Map<number, string>();
function javaOverview() {
  return {
    systemIgnored: false,
    entries: javas.map((j) => ({
      ...j,
      chosen: javaDefaults.get(j.major) === j.id,
      auto: javas.find((other) => other.major === j.major)?.id === j.id,
    })),
  };
}

// 拖文件进窗口：控制台里 `__previewDrop()` 模拟拖进来一批，`__previewDrop("pack")` 只拖一个整合包，
// `__previewHover(true)` 显示「松手导入」那一层
const droppedSample = [
  {
    token: "d-ok",
    name: "jei-1.21.1-neoforge-19.21.jar",
    kind: "mod",
    problem: null,
    title: "Just Enough Items",
    version: "19.21.0",
    icon: cobblemonIcon,
    size: 1_400_000,
  },
  {
    token: "d-fabric",
    name: "sodium-fabric-0.6.5.jar",
    kind: "mod",
    problem: null,
    title: "Sodium",
    version: "0.6.5",
    icon: null,
    size: 1_100_000,
  },
  {
    token: "d-old",
    name: "jade-1.21.1-15.9.jar",
    kind: "mod",
    problem: null,
    title: "Jade",
    version: "15.9.0",
    icon: null,
    size: 820_000,
  },
  {
    token: "d-same",
    name: "appleskin-neoforge-3.0.5.jar",
    kind: "mod",
    problem: null,
    title: "AppleSkin",
    version: "3.0.5",
    icon: null,
    size: 120_000,
  },
  {
    token: "d-pack",
    name: "Faithful 32x.zip",
    kind: "resourcePack",
    problem: null,
    title: null,
    version: null,
    icon: null,
    size: 6_200_000,
  },
  {
    token: "d-data",
    name: "更多合成表.zip",
    kind: "dataPack",
    problem: null,
    title: null,
    version: null,
    icon: null,
    size: 40_000,
  },
  {
    token: "d-world",
    name: "空岛生存.zip",
    kind: "world",
    problem: null,
    title: "空岛生存",
    version: null,
    icon: null,
    size: 3_500_000,
  },
  {
    token: "d-installer",
    name: "neoforge-21.1.265-installer.jar",
    kind: "unknown",
    problem: "installer",
    title: null,
    version: null,
    icon: null,
    size: 9_800_000,
  },
  {
    token: "d-image",
    name: "截图.png",
    kind: "unknown",
    problem: "image",
    title: null,
    version: null,
    icon: null,
    size: 800_000,
  },
  {
    token: "d-minecraft",
    name: ".minecraft",
    kind: "unknown",
    problem: "gameDir",
    title: null,
    version: null,
    icon: null,
    size: 0,
    folder: { kind: "gameRoot", root: foundRoots[0], linked: false },
  },
];
// `__previewDrop("minecraft")`：只拖进来一个别家的 .minecraft（不弹「装到哪」，直接问挂不挂）
const droppedGameRoot = droppedSample.filter((d) => d.token === "d-minecraft");
// `__previewDrop("bundle")`：一包东西（懒人包），先「正在看看是什么」一秒，再拆出四十多样
const bundleName = "懒人包-生存必备.zip";
const droppedBundle = [
  ...Array.from({ length: 40 }, (_, i) => ({
    token: `b-mod-${i}`,
    name: `mod-${i + 1}-neoforge-1.21.1.jar`,
    kind: "mod",
    problem: null,
    title: `示例模组 ${i + 1}`,
    version: `1.${i}.0`,
    icon: i % 3 === 0 ? cobblemonIcon : null,
    size: 300_000 + i * 20_000,
    from: bundleName,
  })),
  {
    token: "b-shader",
    name: "ComplementaryReimagined_r5.9.3.zip",
    kind: "shader",
    problem: null,
    title: null,
    version: null,
    icon: null,
    size: 1_200_000,
    from: bundleName,
  },
  {
    token: "b-world",
    name: "空岛",
    kind: "world",
    problem: null,
    title: "空岛",
    version: null,
    icon: null,
    size: 0,
    from: bundleName,
  },
  {
    token: "b-schematic",
    name: "基地.litematic",
    kind: "schematic",
    problem: null,
    title: null,
    version: null,
    icon: null,
    size: 54_000,
    from: bundleName,
  },
  {
    token: "b-nested",
    name: "备用模组.zip",
    kind: "unknown",
    problem: "nestedBundle",
    title: null,
    version: null,
    icon: null,
    size: 8_000_000,
    from: bundleName,
  },
];
const droppedPack = [
  {
    token: "d-modpack",
    name: "Cobblemon Official Modpack.mrpack",
    kind: "modpack",
    problem: null,
    title: null,
    version: null,
    icon: null,
    size: 2_300_000,
  },
];

/** 下载历史的假数据：几种来源、几种结果，失败的带步骤、能重试 */
const minute = 60_000;
const pastJob = (
  id: string,
  over: Record<string, unknown> & { startedAt: number; finishedAt: number },
) => ({
  id,
  key: `preview:${id}`,
  title: id,
  state: "succeeded",
  progress: { done: 1, total: 1, unit: "items", message: null },
  error: null,
  source: "other",
  instanceId: null,
  iconUrl: null,
  steps: [],
  speed: null,
  eta: null,
  retryable: false,
  downloadSource: null,
  fallbacks: 0,
  ...over,
});
let previewHistory = (() => {
  const now = Date.now();
  const hour = 60 * minute;
  return [
    pastJob("past-sodium", {
      key: "content:验收测试:AANobbMI",
      title: "安装 Sodium",
      source: "modrinth",
      instanceId: "验收测试",
      progress: { done: 1_292_934, total: 1_292_934, unit: "bytes", message: "放进实例" },
      startedAt: now - 42 * minute,
      finishedAt: now - 42 * minute + 9000,
      steps: [
        { label: "下载", at: now - 42 * minute },
        { label: "放进实例", at: now - 42 * minute + 8000 },
      ],
      retryable: true,
    }),
    pastJob("past-pack", {
      key: "modpack-import:modrinth:1KVo5zza",
      title: "安装整合包 Fabulously Optimized",
      state: "failed",
      source: "modrinth",
      error: "有 3 个文件下载失败（服务器返回 503）",
      progress: {
        done: 61_000_000,
        total: 98_000_000,
        unit: "bytes",
        message: "下载整合包文件 81/84",
      },
      startedAt: now - 3 * hour,
      finishedAt: now - 3 * hour + 4 * minute,
      steps: [
        { label: "下载整合包", at: now - 3 * hour },
        { label: "检查整合包", at: now - 3 * hour + 20_000 },
        { label: "下载加载器依赖", at: now - 3 * hour + 30_000 },
        { label: "下载整合包文件", at: now - 3 * hour + 90_000 },
      ],
      retryable: true,
    }),
    pastJob("past-create", {
      key: "instance-create:新的生存",
      title: "新建实例 新的生存",
      state: "cancelled",
      source: "game",
      progress: {
        done: 120_000_000,
        total: 640_000_000,
        unit: "bytes",
        message: "补全游戏文件 812/3998",
      },
      startedAt: now - 26 * hour,
      finishedAt: now - 26 * hour + 2 * minute,
      steps: [{ label: "补全游戏文件", at: now - 26 * hour + 5000 }],
      retryable: true,
      downloadSource: "mirror",
      fallbacks: 3,
    }),
    pastJob("past-interrupted", {
      key: "modpack-import:file:我的整合包",
      title: "导入整合包 我的整合包",
      state: "interrupted",
      source: "local",
      progress: {
        done: 30_000_000,
        total: 210_000_000,
        unit: "bytes",
        message: "下载整合包文件 40/212",
      },
      startedAt: now - 30 * hour,
      finishedAt: now - 30 * hour + 3 * minute,
      steps: [{ label: "下载整合包文件", at: now - 30 * hour + 3 * minute }],
      retryable: true,
      downloadSource: "official",
    }),
    pastJob("past-java", {
      key: "java:21",
      title: "下载 Java 21",
      source: "java",
      progress: { done: 52_000_000, total: 52_000_000, unit: "bytes", message: null },
      startedAt: now - 48 * hour,
      finishedAt: now - 48 * hour + 40_000,
      steps: [{ label: "下载 Java 21", at: now - 48 * hour }],
      retryable: true,
      downloadSource: "official",
    }),
    pastJob("past-gone", {
      key: "content:旧实例:abc",
      title: "安装 Lithium",
      source: "modrinth",
      instanceId: "旧实例",
      progress: { done: 700_000, total: 700_000, unit: "bytes", message: null },
      startedAt: now - 60 * hour,
      finishedAt: now - 60 * hour + 4000,
    }),
    pastJob("past-backup", {
      key: "backup:验收测试:新的世界",
      title: "备份世界「新的世界」",
      source: "local",
      instanceId: "验收测试",
      progress: { done: 31_000_000, total: 31_000_000, unit: "bytes", message: null },
      startedAt: now - 72 * hour,
      finishedAt: now - 72 * hour + 6000,
    }),
  ];
})();

// 地址带 `?onboarding` 走首次引导，带 `?authUnavailable` 模拟 Microsoft 登录不可用。
// 假皮肤是画出来的（头、身体、手臂、腿各一个颜色），只在预览页用
function fakeSkin(arm: [number, number, number], slim: boolean): string {
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 64;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const fill = (rgb: [number, number, number], x: number, y: number, w: number, h: number) => {
    ctx.fillStyle = `rgb(${rgb.join(",")})`;
    ctx.fillRect(x, y, w, h);
  };
  fill([236, 190, 150], 0, 0, 32, 16);
  fill([70, 50, 40], 0, 0, 32, 4);
  fill([70, 50, 40], 8, 8, 8, 2);
  fill([30, 30, 120], 9, 12, 2, 1);
  fill([30, 30, 120], 13, 12, 2, 1);
  fill([40, 170, 90], 16, 16, 24, 16);
  fill([40, 50, 140], 0, 16, 16, 16);
  fill([40, 50, 140], 16, 48, 16, 16);
  fill(arm, 40, 16, slim ? 14 : 16, 16);
  fill(arm, 32, 48, slim ? 14 : 16, 16);
  return canvas.toDataURL("image/png");
}
const previewSkins: Record<string, string> = {
  "skin-b": fakeSkin([250, 140, 20], true),
};
const previewParams = new URLSearchParams(location.search);
const previewUpdateParam = previewParams.get("updater");
const previewUpdateMode: UpdateMode =
  previewUpdateParam === "installed" || previewUpdateParam === "portable"
    ? previewUpdateParam
    : "disabled";
const previewUpdate: UpdateOffer = {
  version: "0.7.0",
  notes: "修复了启动时的显示问题。\n改善了实例列表的滚动表现。",
};
let previewOnboarding = previewParams.has("onboarding");
const previewMicrosoftAvailable = !previewParams.has("authUnavailable");
let previewAuthError: string | null = null;
let cancelPreviewLogin: (() => void) | null = null;
const microsoftAccount: AccountView = {
  id: "acc-microsoft",
  name: "Kokoa77",
  uuid: "1c1a3a4e-0000-4000-8000-000000000003",
  hasSkin: true,
  skinModel: "slim",
  skinHash: "skin-b",
};
let previewAccounts: AccountView[] =
  previewOnboarding || !previewMicrosoftAvailable ? [] : [microsoftAccount];
let previewSelected: string | null = previewAccounts[0]?.id ?? null;
const previewAccountState = (): AccountsState => ({
  selected: previewSelected,
  accounts: previewAccounts,
  onboarding: previewOnboarding,
  microsoftAvailable: previewMicrosoftAvailable,
});
const previewAuthFailure = (code: string) => ({
  code,
  message: "预览页模拟 Microsoft 登录状态",
  detail: null,
});
const accountHandlers: Record<string, (args: Record<string, unknown>) => unknown> = {
  accounts_list: previewAccountState,
  accounts_skin: (args) => {
    const hash = previewAccounts.find((a) => a.id === args.accountId)?.skinHash;
    return hash ? (previewSkins[hash] ?? null) : null;
  },
  accounts_select: (args) => {
    const account = previewAccounts.find((a) => a.id === args.accountId);
    if (!account) throw { code: "accounts.not_found", message: "找不到这个账户", detail: null };
    previewSelected = account.id;
    return null;
  },
  accounts_remove: (args) => {
    previewAccounts = previewAccounts.filter((a) => a.id !== args.accountId);
    if (previewSelected === args.accountId) previewSelected = previewAccounts[0]?.id ?? null;
    return null;
  },
  accounts_microsoft_login: () => {
    if (!previewMicrosoftAvailable) throw previewAuthFailure("auth.app_unavailable");
    if (cancelPreviewLogin) throw previewAuthFailure("auth.login_in_progress");
    return new Promise<AccountsState>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        cancelPreviewLogin = null;
        if (previewAuthError) {
          reject(previewAuthFailure(previewAuthError));
          return;
        }
        if (!previewAccounts.some((account) => account.id === microsoftAccount.id)) {
          previewAccounts = [...previewAccounts, microsoftAccount];
        }
        previewSelected = microsoftAccount.id;
        resolve(previewAccountState());
      }, 800);
      cancelPreviewLogin = () => {
        window.clearTimeout(timer);
        cancelPreviewLogin = null;
        reject(previewAuthFailure("auth.cancelled"));
      };
    });
  },
  accounts_microsoft_cancel: () => {
    if (!cancelPreviewLogin) return false;
    cancelPreviewLogin();
    return true;
  },
  accounts_finish_onboarding: () => {
    previewOnboarding = false;
    return null;
  },
  accounts_restart_onboarding: () => {
    previewOnboarding = true;
    return null;
  },
};

const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
  ...accountHandlers,
  // ?updater=installed 或 ?updater=portable，预览无需联网和安装。
  updates_mode: () => previewUpdateMode,
  updates_check: () => (previewUpdateMode === "installed" ? previewUpdate : null),
  updates_install: async (args) => {
    if (previewUpdateMode !== "installed" || args.version !== previewUpdate.version) {
      throw { code: "updates.recheck", message: "没有匹配的更新，请重新检查。", detail: null };
    }
    const total = 32_000_000;
    for (const fraction of [0.25, 0.5, 0.75, 1]) {
      await new Promise((resolve) => window.setTimeout(resolve, 250));
      emitPreview("update-progress", { downloaded: total * fraction, total });
    }
    return null;
  },
  jobs_history: () => previewHistory,
  jobs_clear_history: () => {
    previewHistory = [];
    return null;
  },
  jobs_remove: (args) => {
    previewHistory = previewHistory.filter((j) => j.id !== args.id);
    return true;
  },
  jobs_retry: (args) => {
    const old = previewHistory.find((j) => j.id === args.id);
    previewHistory = previewHistory.filter((j) => j.id !== args.id);
    return fakeJob(old?.key ?? "content:验收测试:retry", old?.title ?? "重试");
  },
  net_speed_last: () => null,
  net_speed_test: () =>
    new Promise((resolve) =>
      setTimeout(
        () => resolve({ officialMs: 1180, mirrorMs: 2340, pick: "official", at: Date.now() }),
        1200,
      ),
    ),
  net_doh_test: () =>
    new Promise((resolve) =>
      setTimeout(
        () =>
          resolve([
            { name: "阿里 DoH", ms: 32, addresses: ["150.171.110.100"], error: null },
            { name: "Cloudflare DoH", ms: null, addresses: [], error: "超时" },
            {
              name: "系统 DNS",
              ms: 12,
              addresses: ["150.171.110.100", "150.171.109.100"],
              error: null,
            },
          ]),
        900,
      ),
    ),
  net_diagnostics: () =>
    new Promise((resolve) =>
      setTimeout(
        () =>
          resolve([
            {
              id: "microsoftAuth",
              host: "login.microsoftonline.com",
              status: "ok",
              httpStatus: 200,
              latencyMs: 148,
              error: null,
            },
            {
              id: "mojang",
              host: "piston-meta.mojang.com",
              status: "ok",
              httpStatus: 200,
              latencyMs: 216,
              error: null,
            },
            {
              id: "bmclapi",
              host: "bmclapi2.bangbang93.com",
              status: "ok",
              httpStatus: 200,
              latencyMs: 62,
              error: null,
            },
            {
              id: "modrinth",
              host: "api.modrinth.com",
              status: "ok",
              httpStatus: 200,
              latencyMs: 184,
              error: null,
            },
            {
              id: "curseforge",
              host: "api.curseforge.com",
              status: "restricted",
              httpStatus: 403,
              latencyMs: 295,
              error: "HTTP 403",
            },
            {
              id: "mcim",
              host: "mod.mcimirror.top",
              status: "ok",
              httpStatus: 200,
              latencyMs: 54,
              error: null,
            },
          ]),
        700,
      ),
    ),
  claim_folder_info: () => ({ path: String.raw`C:\Users\玩家\Downloads`, custom: false }),
  claim_folder_choose: () => ({ path: String.raw`D:\下载`, custom: true }),
  claim_folder_reset: () => ({ path: String.raw`C:\Users\玩家\Downloads`, custom: false }),
  drops_check: (args) =>
    (args.tokens as string[]).map((token) => ({
      token,
      fit:
        token === "d-fabric" || token === "b-mod-7" || token === "b-mod-23"
          ? { kind: "wrongLoader", needs: ["fabric", "quilt"] }
          : { kind: "fits" },
      duplicate:
        token === "d-old"
          ? { kind: "otherVersion", file: "jade-1.21.1-15.8.jar", version: "15.8.2" }
          : token === "d-same"
            ? { kind: "same" }
            : null,
    })),
  drops_install: (args) => {
    const id = `job-${Date.now()}`;
    const key = `drop:${String(args.instanceId)}:${id}`;
    const n = (args.choices as unknown[]).length;
    const startedAt = Date.now();
    const job = (state: string, done: number) => ({
      id,
      key,
      title: `装进 ${String(args.instanceId)}：${n} 个文件`,
      state,
      progress: { done, total: n, unit: "items", message: null },
      error: null,
      startedAt,
      finishedAt: state === "running" ? null : Date.now(),
    });
    // 慢慢走完，看得到任务中心里的进度（一大包也在 3 秒左右走完）
    const step = Math.min(500, 3000 / Math.max(n, 1));
    for (let i = 0; i < n; i += 1) {
      setTimeout(() => emitPreview("job-updated", job("running", i)), 50 + i * step);
    }
    setTimeout(() => emitPreview("job-updated", job("succeeded", n)), 50 + n * step);
    return id;
  },
  drops_modpack: () => ({
    token: "pack-1",
    preview: {
      format: "modrinth",
      name: "Cobblemon Official Modpack",
      version: "1.6.1",
      summary: "宝可梦风格的整合包",
      gameVersion: "1.21.1",
      loader: { kind: "neoforge", version: "21.1.209" },
      fileCount: 142,
      downloadBytes: 380_000_000,
      overrideCount: 36,
      optionalFiles: [],
      manualFiles: [],
      fromModrinth: 0,
      recommendedMemoryMb: null,
    },
    readiness: {
      gameInstalled: true,
      loaderInstalled: true,
      javaRequired: 21,
      javaFound: "21.0.7",
    },
  }),
  drops_discard: () => null,
  modpack_discard: () => null,
  app_info: () => ({ version: "0.6.0（预览）", backdrop: "solid", memoryMb: 32768 }),
  memory_status: () => {
    // 像真的一样每次读都有一点起伏
    availableMb = Math.max(1500, availableMb + Math.round((Math.random() - 0.5) * 120));
    const spare = Math.floor((availableMb - 1024) / 256) * 256;
    return {
      totalMb: TOTAL_MB,
      availableMb,
      autoMaxMb: Math.max(Math.min(16384, TOTAL_MB / 2, spare), 2048),
    };
  },
  memory_check: (args) => memoryCheck(String(args.instanceId)),
  launch_preflight: () => ({ issues: [], checkedMods: 12, elapsedMs: 14 }),
  memory_programs: () => [
    { name: "Google Chrome", memoryMb: 2150, processes: 23 },
    { name: "Adobe Photoshop 2025", memoryMb: 1840, processes: 2 },
    { name: "Visual Studio Code", memoryMb: 980, processes: 9 },
    { name: "QQ", memoryMb: 620, processes: 5 },
  ],
  memory_clean: async () => {
    await new Promise((resolve) => setTimeout(resolve, 1500));
    availableMb += 1230;
    return { freedMb: 1230, availableMb };
  },
  java_list: () => javaOverview(),
  java_add: () => null,
  java_install: (args) => {
    const major = Number(args.major);
    const id = `job-java-${major}-${Date.now()}`;
    const startedAt = Date.now();
    const total = 99_000_000;
    const job = (state: string, done: number) => ({
      id,
      key: `java:${major}`,
      title: `下载 Java ${major}`,
      state,
      progress: { done, total, unit: "bytes", message: `下载 Java ${major}` },
      error: null,
      startedAt,
      finishedAt: state === "running" ? null : Date.now(),
    });
    for (let i = 0; i < 10; i += 1) {
      setTimeout(() => emitPreview("job-updated", job("running", (total * i) / 10)), 50 + i * 200);
    }
    setTimeout(() => {
      javas = [
        ...javas,
        {
          id: `dl-${major}`,
          major,
          version: major === 8 ? "1.8.0_462" : `${major}.0.8`,
          vendor: "Microsoft",
          is64bit: true,
          origin: "downloaded" as const,
          location: String.raw`C:\Games\Lumi\.minecraft\.mcl-java\java-runtime-${major}`,
        },
      ].sort((a, b) => b.major - a.major);
      emitPreview("job-updated", job("succeeded", total));
    }, 2100);
    return id;
  },
  java_set_default: (args) => {
    const major = Number(args.major);
    if (args.id) javaDefaults.set(major, String(args.id));
    else javaDefaults.delete(major);
    return javaOverview();
  },
  java_remove: (args) => {
    javas = javas.filter((j) => j.id !== args.id);
    return javaOverview();
  },
  java_delete: (args) => {
    javas = javas.filter((j) => j.id !== args.id);
    return javaOverview();
  },
  app_ready: () => null,
  app_set_ui_language: () => null,
  app_log: () => null,
  settings_get: () => settings,
  settings_set: (args) => {
    settings = args.settings as typeof settings;
    return settings;
  },
  settings_export: () => true,
  settings_import: () => settings,
  storage_scan: () => {
    const instancesBytes = 1_420_000_000;
    const sharedGameBytes = 980_000_000;
    const javaBytes = 410_000_000;
    const backupsBytes = Array.from(worldBackups.values())
      .flat()
      .reduce((sum, item) => sum + item.size, 0);
    const cacheBytes = previewCacheBytes;
    return {
      instancesBytes,
      sharedGameBytes,
      javaBytes,
      backupsBytes,
      cacheBytes,
      totalBytes: instancesBytes + sharedGameBytes + javaBytes + backupsBytes + cacheBytes,
      cacheFiles: previewCacheFiles,
      pathHasBang: false,
    };
  },
  storage_clean_cache: () => {
    const result = { freedBytes: previewCacheBytes, removedFiles: previewCacheFiles };
    previewCacheBytes = 0;
    previewCacheFiles = 0;
    return result;
  },
  storage_unused_versions: () => previewUnusedVersions.map((v) => ({ ...v })),
  storage_clean_unused_versions: (args) => {
    const ids = new Set((args.ids as string[]) ?? []);
    let freedBytes = 0;
    let removedCount = 0;
    previewUnusedVersions = previewUnusedVersions.filter((item) => {
      if (ids.has(item.id)) {
        freedBytes += item.bytes;
        removedCount += 1;
        return false;
      }
      return true;
    });
    return { freedBytes, removedCount };
  },
  backups_dir_info: () => ({
    path: settings.worlds.backupDir,
    custom: settings.worlds.backupDir !== null,
  }),
  backups_dir_choose: () => {
    settings.worlds.backupDir = String.raw`E:\LumiBackups`;
    return {
      info: { path: settings.worlds.backupDir, custom: true },
      movedFiles: 3,
      movedBytes: 137_100_000,
    };
  },
  backups_dir_reset: () => {
    settings.worlds.backupDir = null;
    return {
      info: { path: null, custom: false },
      movedFiles: 3,
      movedBytes: 137_100_000,
    };
  },
  sync_overview: () => {
    const owned = instances.filter((i) => !i.linked);
    const detached = owned.filter((i) => instanceOverrides.get(i.id)?.syncDisabled).length;
    return {
      participatingInstances: owned.length - detached,
      detachedInstances: detached,
      linkedInstances: instances.length - owned.length,
      sharedOptionsKeys: Object.keys(previewSharedOptions).length,
      sharedServers: previewSharedServers.length,
      sharedResourcePacks: previewSharedPacks.resourcePacks?.length ?? 0,
      sharedShaderPacks: previewSharedPacks.shaderPacks?.length ?? 0,
      sharedCommands: previewSharedCommands.length,
      sharedHotbarBytes: previewSharedHotbarBytes,
    };
  },
  sync_preview: () => ({
    sharedHasData:
      Object.keys(previewSharedOptions).length > 0 ||
      previewSharedServers.length > 0 ||
      previewSharedCommands.length > 0 ||
      previewSharedHotbarBytes !== null,
    instances: instances
      .filter((i) => !i.linked && !instanceOverrides.get(i.id)?.syncDisabled)
      .map((i, idx) => ({
        instanceId: i.id,
        instanceName: i.name,
        optionsConflicts: idx === 0 ? 0 : 3,
        serversDiff: idx === 1 ? 1 : 0,
        packsPending: settings.sync.resourcePacks ? 1 : 0,
        commandsDiff: settings.sync.commandHistory ? 1 : 0,
        hotbarDiff: Boolean(settings.sync.hotbar),
        willBackupFirst: idx > 0,
      })),
  }),
  sync_apply: () => ({
    updatedInstances: 3,
    backupsCreated: 2,
    optionsWritten: 9,
    serversMerged: 2,
    packsSynced: settings.sync.resourcePacks ? 3 : 0,
    commandsMerged: settings.sync.commandHistory ? 2 : 0,
    hotbarsSynced: settings.sync.hotbar ? 2 : 0,
  }),
  sync_open_shared_folder: () => null,
  sync_shared_options_get: () => ({ ...previewSharedOptions }),
  sync_shared_options_set: (args) => {
    previewSharedOptions = { ...((args.entries as Record<string, string>) ?? {}) };
    return { ...previewSharedOptions };
  },
  sync_shared_servers_list: () => previewSharedServers.map((s) => ({ ...s })),
  sync_shared_servers_add: (args) => {
    const address = String(args.address ?? "").trim();
    const name = String(args.name ?? "").trim() || address;
    if (!previewSharedServers.some((s) => s.address.toLowerCase() === address.toLowerCase())) {
      previewSharedServers = [...previewSharedServers, { name, address }];
    }
    return previewSharedServers.map((s) => ({ ...s }));
  },
  sync_shared_servers_remove: (args) => {
    const address = String(args.address ?? "").toLowerCase();
    previewSharedServers = previewSharedServers.filter((s) => s.address.toLowerCase() !== address);
    return previewSharedServers.map((s) => ({ ...s }));
  },
  sync_shared_packs_list: (args) =>
    (previewSharedPacks[String(args.kind)] ?? []).map((p) => ({ ...p })),
  sync_shared_packs_remove: (args) => {
    const kind = String(args.kind);
    const name = String(args.name);
    previewSharedPacks[kind] = (previewSharedPacks[kind] ?? []).filter((p) => p.name !== name);
    return previewSharedPacks[kind].map((p) => ({ ...p }));
  },
  sync_shared_commands_get: () => [...previewSharedCommands],
  sync_shared_commands_set: (args) => {
    previewSharedCommands = [...((args.lines as string[]) ?? [])];
    return [...previewSharedCommands];
  },
  sync_shared_hotbar_clear: () => {
    previewSharedHotbarBytes = null;
    return null;
  },
  sync_import_from_instance: (args) => {
    if (args.importHotbar) previewSharedHotbarBytes = 4_096;
    return null;
  },
  game_dir_info: () => ({ path: String.raw`C:\Games\Lumi\.minecraft`, locked: true }),
  catalog_game_versions: () => ["26.3", "1.21.1", "1.20.1", "1.16.5", "1.12.2", "1.1"],
  catalog_loader_versions: (args) => {
    const mc = String(args.gameVersion);
    switch (args.kind) {
      case "neoforge":
        if (mc === "26.3") return [{ version: "26.3.0.113", stable: true }];
        if (mc === "1.20.1") return [{ version: "47.1.106", stable: true }];
        return mc === "1.21.1" ? [{ version: "21.1.265", stable: true }] : [];
      case "forge": {
        const versions: Record<string, string> = {
          "26.3": "66.1.8",
          "1.21.1": "52.1.20",
          "1.20.1": "47.4.28",
          "1.16.5": "36.2.34",
        };
        const version = versions[mc];
        return version ? [{ version, stable: true }] : null;
      }
      case "fabric":
        return [{ version: "0.16.10", stable: true }];
      case "quilt":
        return mc === "1.12.2" ? [] : [{ version: "0.24.0", stable: true }];
      default:
        return [];
    }
  },
  instances_create: (args) => {
    const name = String(args.name);
    const gameVersion = String(args.gameVersion);
    const loader = args.loader as { kind: string; version: string } | null;
    const labels: Record<string, string> = {
      neoforge: "NeoForge",
      forge: "Forge",
      fabric: "Fabric",
      quilt: "Quilt",
    };
    return fakeJob(`instance-create:${name}`, `新建实例 ${name}`, () => {
      instances.unshift({
        id: name,
        name,
        gameVersion,
        loader: loader ? `${labels[loader.kind] ?? loader.kind} ${loader.version}` : null,
        pack: null,
        createdAt: Date.now(),
        lastPlayed: null,
        group: null,
        pinnedAt: null,
      });
    });
  },
  jobs_list: () => [],
  launch_sessions: () => [],
  launch_log: () => [],
  // 每次给一份副本：真的后端每次都是新数据，界面靠这个知道变了
  instances_list: () => instances.map((i) => ({ ...i })),
  instances_open_folder: () => null,
  instances_settings_get: (args) => {
    const id = String(args.instanceId);
    return {
      name: instances.find((i) => i.id === id)?.name ?? id,
      overrides: instanceOverrides.get(id) ?? noOverrides,
      requiredJava: 21,
      autoJavaId: javaOverview().entries.find((e) => e.major === 21 && e.auto)?.id ?? null,
    };
  },
  instances_settings_set: (args) => {
    const id = String(args.instanceId);
    const instance = instances.find((i) => i.id === id);
    if (instance && String(args.name).trim()) instance.name = String(args.name).trim();
    instanceOverrides.set(id, args.overrides as Overrides);
    return {
      name: instance?.name ?? id,
      overrides: args.overrides,
      requiredJava: 21,
      autoJavaId: javaOverview().entries.find((e) => e.major === 21 && e.auto)?.id ?? null,
    };
  },
  instances_export_preview: async (args) => {
    await new Promise((resolve) => setTimeout(resolve, 900));
    return exportPreview(String(args.instanceId));
  },
  instances_export: (args) => {
    const options = args.options as { name: string; version: string };
    return fakeJob(
      `instance-export:${args.instanceId}`,
      `导出整合包 ${options.name}-${options.version || "1.0.0"}.mrpack`,
    );
  },
  instances_reveal_export: () => null,
  instances_copy: (args) => {
    const from = instances.find((i) => i.id === args.instanceId);
    const name = String(args.name);
    const jobId = fakeJob(`instance-copy:${args.instanceId}`, `复制实例 ${name}`, () => {
      if (from)
        instances.push({
          ...from,
          id: name,
          name,
          createdAt: Date.now(),
          lastPlayed: null,
          pinnedAt: null,
        });
    });
    return { jobId, instanceId: name };
  },
  linked_roots_list: () => linkedRoots.map((r) => ({ ...r })),
  linked_roots_discover: () => foundRoots.map((r) => ({ ...r })),
  linked_roots_add: (args) => {
    const index = foundRoots.findIndex((r) => r.id === args.suggestionId);
    const [root] = index >= 0 ? foundRoots.splice(index, 1) : [];
    if (root) {
      linkedRoots.push(root);
      return { ...root };
    }
    // 已经挂上的再挂一次：移除过的加回来
    const known = linkedRoots.find((r) => r.id === args.suggestionId);
    if (known) known.hidden = 0;
    return known ? { ...known } : null;
  },
  // 设置里「添加文件夹」：假装选到了一个 Prism 实例（打开导入窗口）
  linked_roots_pick: () => ({ kind: "foreign", instances: [{ ...pickedForeign }] }),
  linked_roots_remove: (args) => {
    const index = linkedRoots.findIndex((r) => r.id === args.id);
    if (index >= 0) foundRoots.push(...linkedRoots.splice(index, 1));
    return 2;
  },
  linked_roots_restore: (args) => {
    const root = linkedRoots.find((r) => r.id === args.id);
    if (!root) return null;
    root.hidden = 0;
    return { ...root };
  },
  foreign_list: () => foreignInstances.map((i) => ({ ...i })),
  // 导入窗口里「选择文件夹」：假装选到了意外农场的 .minecraft（还没挂上时问一句，挂上了说一声）
  foreign_pick: () => {
    const farm = foundRoots.find((r) => r.id === "root-farm");
    if (farm) return { kind: "gameRoot", root: { ...farm }, linked: false };
    const linked = linkedRoots.find((r) => r.id === "root-farm");
    return linked ? { kind: "gameRoot", root: { ...linked }, linked: true } : null;
  },
  foreign_import: (args) => {
    const found = [...foreignInstances, pickedForeign].find((i) => i.id === args.id);
    return fakeJob(`instance-create:import:${String(args.id)}`, `导入实例 ${found?.name ?? ""}`);
  },
  instances_delete: (args) => {
    const index = instances.findIndex((i) => i.id === args.instanceId);
    if (index >= 0) instances.splice(index, 1);
    return null;
  },
  worlds_recent: () => recent.map((r) => ({ ...r, world: withBackground(r.instanceId, r.world) })),
  worlds_recent_servers: () => previewRecentServers,
  worlds_list: (args) => {
    const id = String(args.instanceId);
    const entry = recent.find((r) => r.instanceId === id);
    if (!entry) return { quickPlay: true, worlds: [] };
    return {
      quickPlay: true,
      worlds: [
        entry.world,
        world("创造测试", "创造测试", 1, 30, cobblemonIcon, "icon"),
        { ...world("空岛", "空岛生存", 18, 200, worldShot, "screenshot"), hardcore: true },
        { ...world("旧存档", "旧存档", 120, 900, "", "icon"), imageUrl: null, imageKind: null },
      ].map((w) => withBackground(id, w)),
    };
  },
  worlds_notes_get: (args) =>
    worldNotes.get(backgroundKey(args)) ?? { places: [], todos: [], text: "" },
  worlds_notes_set: (args) => {
    const notes = args.notes as FakeNotes;
    const cleaned = {
      places: notes.places ?? [],
      todos: (notes.todos ?? []).filter((t) => t.text.trim()),
      text: notes.text ?? "",
    };
    worldNotes.set(backgroundKey(args), cleaned);
    return cleaned;
  },
  backups_list: (args) => worldBackups.get(backgroundKey(args)) ?? [],
  backups_create: (args) =>
    fakeJob(`backup:${String(args.instanceId)}:${String(args.folder)}`, "备份世界", () => {
      const key = backgroundKey(args);
      worldBackups.set(key, [
        { id: String(Date.now()), at: Date.now(), reason: "manual", size: 48_300_000 },
        ...(worldBackups.get(key) ?? []),
      ]);
    }),
  backups_restore: (args) =>
    fakeJob(`backup:${String(args.instanceId)}:${String(args.folder)}`, "恢复世界", () => {
      const key = backgroundKey(args);
      worldBackups.set(key, [
        { id: String(Date.now()), at: Date.now(), reason: "beforeRestore", size: 48_400_000 },
        ...(worldBackups.get(key) ?? []),
      ]);
    }),
  backups_delete: (args) => {
    const key = backgroundKey(args);
    worldBackups.set(
      key,
      (worldBackups.get(key) ?? []).filter((b) => b.id !== args.backupId),
    );
    return null;
  },
  backups_reveal: () => null,
  backups_decide: (args) =>
    args.auto
      ? fakeJob(`backup:${String(args.instanceId)}:${String(args.folder)}`, "备份世界")
      : null,
  backups_usage: () => {
    const all = [...worldBackups.values()].flat();
    return { bytes: all.reduce((sum, b) => sum + b.size, 0), count: all.length };
  },
  backups_clear: () => {
    const all = [...worldBackups.values()].flat();
    worldBackups.clear();
    return all.reduce((sum, b) => sum + b.size, 0);
  },
  worlds_background_pick: (args) => {
    backgrounds.set(backgroundKey(args), { url: standinArt, fit: "contain" });
    return true;
  },
  worlds_background_fit: (args) => {
    const current = backgrounds.get(backgroundKey(args));
    if (current)
      backgrounds.set(backgroundKey(args), { ...current, fit: args.fit as "cover" | "contain" });
    return null;
  },
  worlds_background_clear: (args) => {
    backgrounds.delete(backgroundKey(args));
    return null;
  },
  worlds_auto_covers_list: (args) => {
    const key = backgroundKey(args);
    const locked = lockedAutoCovers.get(key) ?? null;
    const files = ["3.jpg", "2.jpg", "1.jpg"];
    return files.map((file, i) => ({
      file,
      url: worldShot,
      takenAt: now - (i + 1) * 4 * HOUR,
      locked: locked === file,
      active: locked ? locked === file : i === 0,
    }));
  },
  worlds_auto_cover_lock: (args) => {
    lockedAutoCovers.set(backgroundKey(args), String(args.file));
    return null;
  },
  worlds_auto_cover_unlock: (args) => {
    lockedAutoCovers.delete(backgroundKey(args));
    return null;
  },
  worlds_save_share_card: (args) => {
    const a = document.createElement("a");
    a.href = String(args.pngBase64);
    a.download = String(args.suggestedName || "lumi-world-card.png");
    a.click();
    return true;
  },
  content_list: (args) =>
    args.kind === "mod"
      ? mods.map((title, i) => {
          const fileName = `${title.toLowerCase().replace(/ /g, "-")}.jar`;
          const lockKey = `${String(args.instanceId)}/${String(args.kind)}/${fileName}`;
          return {
            kind: "mod",
            fileName,
            title,
            enabled: i !== 2,
            size: 200_000 + i * 50_000,
            projectId: i % 3 === 2 ? null : `mod-${i}`,
            versionId: `ver-${i}-1`,
            versionNumber: `${1 + i}.0.${i}`,
            locked: lockedContent.has(lockKey),
            iconUrl: i % 2 === 0 ? sampleIcons[i % sampleIcons.length] : null,
            source: i % 3 === 2 ? "file" : i % 3 === 1 ? "curseForge" : "modrinth",
            isFolder: false,
            author: i % 3 === 2 ? null : (sampleProjects[i]?.[2] ?? "someone"),
            authorUrl: i % 3 === 0 ? `https://modrinth.com/user/${sampleProjects[i]?.[2]}` : null,
            pageUrl: i % 3 === 2 ? null : `https://modrinth.com/project/mod-${i}`,
            modified: now - (i + 1) * 7 * HOUR,
            titleZh: previewZh[title] ?? null,
            twinId: null,
          };
        })
      : [],
  content_set_locked: (args) => {
    const lockKey = `${String(args.instanceId)}/${String(args.kind)}/${String(args.fileName)}`;
    if (args.locked) lockedContent.add(lockKey);
    else lockedContent.delete(lockKey);
    return Boolean(args.locked);
  },
  content_switch_version: (args) => {
    const update = args.update as { title: string; versionNumber: string };
    return fakeJob(
      `content:${String(args.instanceId)}:update`,
      `切换 ${update.title} 到 ${update.versionNumber}`,
    );
  },
  content_resolve_version: async (args) => {
    await new Promise((resolve) => setTimeout(resolve, 400));
    return [
      {
        projectId: String(args.projectId),
        versionId: String(args.versionId),
        title: `项目 ${String(args.projectId)}`,
        versionNumber: String(args.versionId),
        fileName: `${String(args.projectId)}-${String(args.versionId)}.jar`,
        size: 1_200_000,
        kind: args.kind,
        iconUrl: null,
        reason: "requested",
        source: args.source,
        delivery: "direct",
        pageUrl: null,
      },
    ];
  },
  discover_versions: (args) => {
    const source = args.source === "curseForge" ? "curseForge" : "modrinth";
    const projectId = String(args.projectId);
    const gv = args.gameVersion ? String(args.gameVersion) : "1.21.1";
    const loader = args.loader ? String(args.loader) : "neoforge";
    return [
      {
        id: "ver-latest",
        projectId,
        source,
        name: "2.1.0",
        versionNumber: "2.1.0+mc1.21.1",
        releaseType: "release",
        gameVersions: [gv],
        loaders: [loader],
        published: new Date(now - 6 * HOUR).toISOString(),
        downloads: 48200,
        changelog:
          source === "modrinth"
            ? "### 更新内容\n- 修复 1.21.1 NeoForge 兼容性问题\n- 优化区块渲染帧率"
            : null,
        changelogFormat: source === "modrinth" ? "markdown" : "html",
        fileName: `${projectId}-2.1.0.jar`,
        fileSize: 842_000,
        delivery: "direct",
        pageUrl: null,
      },
      {
        id: "ver-0-1",
        projectId,
        source,
        name: "1.0.0",
        versionNumber: "1.0.0",
        releaseType: "release",
        gameVersions: [gv, "1.21"],
        loaders: [loader],
        published: new Date(now - 30 * 24 * HOUR).toISOString(),
        downloads: 120500,
        changelog: source === "modrinth" ? "首个 1.21.1 正式版发布。" : null,
        changelogFormat: source === "modrinth" ? "markdown" : "html",
        fileName: `${projectId}-1.0.0.jar`,
        fileSize: 790_000,
        delivery: "direct",
        pageUrl: null,
      },
      {
        id: "ver-beta",
        projectId,
        source,
        name: "2.2.0-beta.1",
        versionNumber: "2.2.0-beta.1",
        releaseType: "beta",
        gameVersions: [gv],
        loaders: [loader],
        published: new Date(now - 2 * HOUR).toISOString(),
        downloads: 3200,
        changelog: source === "modrinth" ? "测试版：实验性内存优化。" : null,
        changelogFormat: source === "modrinth" ? "markdown" : "html",
        fileName: `${projectId}-2.2.0-beta.1.jar`,
        fileSize: 860_000,
        delivery: "direct",
        pageUrl: null,
      },
    ];
  },
  discover_version_changelog: () =>
    "<p>CurseForge 版本更新日志：</p><ul><li>修复模组兼容问题</li><li>提升性能</li></ul>",
  content_refresh: (args) => handlers.content_list?.(args),
  content_history: () => [],
  modrinth_search: (args) => fakePage("modrinth", args),
  curseforge_search: (args) => fakePage("curseForge", args),
  modpack_search: (args) =>
    fakePage(args.source === "curseForge" ? "curseForge" : "modrinth", args),
  discover_project: (args) => {
    const id = String(args.projectId);
    const index = Number(id.split("-").pop() ?? 0);
    const hit = fakeHit(index, args.source === "curseForge" ? "curseForge" : "modrinth", "mod");
    return {
      hit,
      body: `## ${hit.title}\n\n${hit.description}\n\n- 支持 NeoForge 1.21.1\n- 装好就能用，不用另外设置\n\n![截图](${worldShot})\n\n更多说明见 [项目主页](https://modrinth.com/)。\n\n<script>alert("这行会被清洗掉")</script>`,
      bodyFormat: "markdown",
      gallery: [
        { url: worldShot, thumbUrl: worldShot, title: "主城夜景" },
        { url: worldShot, thumbUrl: worldShot, title: null },
      ],
      links: [
        { kind: "source", url: "https://github.com/" },
        { kind: "issues", url: "https://github.com/issues" },
        { kind: "discord", url: "https://discord.gg/" },
      ],
      license: "MIT",
      published: new Date(now - 400 * 24 * HOUR).toISOString(),
      followers: 12345,
      gameVersions: [
        "26.3",
        "26.2",
        "26.1.1",
        "26.1",
        "1.21.1",
        "1.21",
        "1.20.1",
        "1.20",
        "1.19.2",
        "1.19",
        "1.18.2",
        "1.18",
        "1.17.1",
        "1.16.5",
        "1.16",
        "1.15.2",
        "1.14.4",
        "1.13.2",
        "1.12.2",
      ],
      loaders: ["neoforge", "fabric"],
      members: [
        { name: hit.author, role: "Owner", avatarUrl: null, url: hit.authorUrl },
        { name: "helper", role: "Maintainer", avatarUrl: null, url: null },
      ],
    };
  },
  favorites_list: () => favoriteItems,
  names_match: (args) => {
    const query = String(args.query ?? "").trim();
    if (!/\p{Script=Han}/u.test(query)) return [];
    return previewNames.filter((n) => n.zh.includes(query) || query.includes(n.zh));
  },
  favorites_toggle: (args) => {
    const hit = args.hit as { source: string; projectId: string };
    const exists = favoriteItems.some(
      (f) =>
        (f.hit as typeof hit).source === hit.source &&
        (f.hit as typeof hit).projectId === hit.projectId,
    );
    favoriteItems = exists
      ? favoriteItems.filter(
          (f) =>
            !(
              (f.hit as typeof hit).source === hit.source &&
              (f.hit as typeof hit).projectId === hit.projectId
            ),
        )
      : [{ kind: String(args.kind), hit, addedAt: Date.now() }, ...favoriteItems];
    return !exists;
  },
  // 解析要一会儿、都带一个共用前置（连着点几个，确认窗一个一个弹）
  content_resolve: async (args) => {
    await new Promise((resolve) => setTimeout(resolve, 900));
    const item = (projectId: string, title: string, reason: string) => ({
      projectId,
      versionId: "v1",
      title,
      versionNumber: "1.0.0",
      fileName: `${projectId}.jar`,
      size: 1_200_000,
      kind: args.kind,
      iconUrl: null,
      reason,
      source: args.source,
      delivery: "direct",
      pageUrl: null,
    });
    return [
      item(String(args.projectId), `项目 ${String(args.projectId)}`, "requested"),
      item("puzzleslib", "Puzzles Lib", "dependency"),
    ];
  },
  content_install: (args) => {
    const plan = args.plan as { projectId: string; title: string }[];
    const first = plan[0];
    return fakeJob(`content:${args.instanceId}:${first?.projectId}`, `安装 ${first?.title}`);
  },
  content_remove: () => null,
  content_remove_many: (args) => (args.fileNames as string[]).length,
  content_set_enabled: () => null,
  content_set_many_enabled: (args) => args.fileNames as string[],
  // 上次改到一半被关掉：preview.html?recovered 看启动时的提示
  content_recovered: () =>
    new URLSearchParams(location.search).has("recovered")
      ? [
          {
            instanceId: "Cobblemon",
            instanceName: "Cobblemon Official Modpack",
            summary: "更新 Sodium 等 3 项",
            ok: true,
            message: null,
          },
          {
            instanceId: "Fabric Pack",
            instanceName: "验收测试",
            summary: "",
            ok: false,
            message:
              "这个实例上次「删除 3 项：Jade、AppleSkin、Mouse Tweaks」没做完，退回时出了问题：「jade.jar」被别的程序占用（可能是游戏或杀毒软件）。关掉可能占着文件的程序（游戏、杀毒软件）后再试",
          },
        ]
      : [],
  content_check_updates: (args) => {
    if (args.kind !== "mod") return { updates: [], checked: 0, unchecked: 0, lookupFailed: false };
    const instanceId = String(args.instanceId);
    const unlocked = mods.filter(
      (title) =>
        !lockedContent.has(`${instanceId}/mod/${title.toLowerCase().replace(/ /g, "-")}.jar`),
    );
    const updates = unlocked.slice(0, 2).map((title, i) => ({
      kind: "mod",
      fileName: `${title.toLowerCase().replace(/ /g, "-")}.jar`,
      title,
      currentVersion: `${1 + i}.0.${i}`,
      versionId: i === 1 ? "901234" : `ver-new-${i}`,
      versionNumber: `${2 + i}.1.0`,
      source: i === 1 ? "curseForge" : "modrinth",
      projectId: `mod-${i}`,
      delivery: "direct",
      pageUrl: null,
    }));
    const checked = Math.max(updates.length, Math.ceil(unlocked.length / 2));
    return {
      updates,
      checked,
      unchecked: Math.max(0, unlocked.length - checked),
      lookupFailed: false,
    };
  },
  content_reveal: () => null,
  // 地图：三张里有一张作者不许第三方下载
  maps_search: (args) => {
    const page = fakePage("curseForge", { ...args, kind: "map" });
    return { ...page, hits: page.hits.map((hit, i) => ({ ...hit, manualOnly: i % 3 === 0 })) };
  },
  maps_install: () => "preview-job",
  servers_search: serverPage,
  servers_list: () => joinedServers,
  servers_status: async (args) => {
    await new Promise((resolve) => setTimeout(resolve, 400 + Math.random() * 600));
    return fakeServerStatus(String(args.address));
  },
  instances_set_group: (args) => {
    const found = instances.find((i) => i.id === args.instanceId);
    const group = typeof args.group === "string" ? args.group.trim().slice(0, 24) : "";
    if (found) found.group = group || null;
    return null;
  },
  instances_set_pinned: (args) => {
    const found = instances.find((i) => i.id === args.instanceId);
    if (found) found.pinnedAt = args.pinned ? (found.pinnedAt ?? Date.now()) : null;
    return null;
  },
  instances_rename_group: (args) => {
    const to = typeof args.to === "string" ? args.to.trim() || null : null;
    let changed = 0;
    for (const i of instances) {
      if (i.group === args.from) {
        i.group = to;
        changed += 1;
      }
    }
    return changed;
  },
  shortcut_create: (args) => {
    const instance = instances.find((i) => i.id === args.instanceId);
    return args.world ? `${instance?.name} · ${args.world}` : (instance?.name ?? "");
  },
  shortcut_take: () => null,
  window_progress: () => null,
  window_show: () => null,
  servers_add: (args) => {
    const address = String(args.address);
    if (joinedServers.some((s) => s.address === address)) return false;
    joinedServers = [{ name: String(args.name), address }, ...joinedServers];
    return true;
  },
  instances_screenshots: (args) => (args.instanceId === "验收测试" ? screenshots : []),
  instances_open_screenshots: () => null,
  worlds_open_folder: () => null,
  launch_start: (args) => {
    if (!previewMicrosoftAvailable) throw previewAuthFailure("auth.app_unavailable");
    const account = previewAccounts.find((item) => item.id === previewSelected);
    if (!account) throw previewAuthFailure("auth.no_account");
    const id = String(args.instanceId);
    if (previewAuthError) {
      const error = previewAuthFailure(previewAuthError);
      window.setTimeout(() => {
        emitPreview("launch-auth-blocked", {
          instanceId: id,
          code: error.code,
          message: error.message,
          world: args.world ?? null,
          serverAddress: args.server ?? null,
          memoryMb: args.memoryMb ?? null,
        });
      }, 250);
      return null;
    }
    fakeSessions.set(id, fakeLaunch(id));
    return null;
  },
  launch_stop: (args) => {
    const id = String(args.instanceId);
    for (const timer of fakeRuns.get(id) ?? []) window.clearTimeout(timer);
    const run = fakeSessions.get(id);
    if (run && Date.now() - run.startedAt < 1500) emitPreview("job-updated", run.job("cancelled"));
    if (run) emitPreview("session-updated", run.session({ kind: "exited", code: 0 }));
    return true;
  },
  launch_records: (args) => (args.instanceId === "验收测试" ? records : []),
  launch_record_log: (args) => {
    const lines = recordLog(String(args.recordId));
    return { lines, total: lines.length, skipped: 0 };
  },
  launch_record_diagnosis: () => "unknown",
  launch_record_diagnostic_export: () => true,
  instances_repair_files: () => "preview-repair",
  launch_record_crash: () =>
    "---- Minecraft Crash Report ----\n// Who set us up the TNT?\n\nTime: 2026-11-12 15:00:00\nDescription: Ticking entity\n\njava.lang.RuntimeException: boom\n\tat crashtest.CrashTest.onTick(CrashTest.java:12)\n",
  launch_record_reveal: () => null,
  launch_record_upload_log: () => ({
    url: "https://mclo.gs/LumiDemo1",
    raw: "https://api.mclo.gs/1/raw/LumiDemo1",
  }),
  claim_list: () => [],
  claim_skip: () => null,
  claim_pick: () => null,
  catalog_neoforge_versions: (args) =>
    args.gameVersion === "1.20.1"
      ? [
          { version: "47.1.106", beta: false },
          { version: "47.1.105", beta: false },
        ]
      : [
          { version: "21.1.244", beta: false },
          { version: "21.1.219", beta: false },
        ],
  content_update: (args) =>
    fakeJob(`update:${String(args.instanceId)}`, `更新 ${String(args.instanceId)} 模组`),
  content_rollback: () => null,
  content_crash_offer: () => null,
  content_restore_snapshot: () => 2,
  game_dir_choose: () => null,
  modpack_pick: () => null,
  modpack_versions: () => [
    {
      id: "pack-ver-latest",
      name: "1.4.0",
      versionNumber: "1.4.0",
      gameVersion: "1.21.1",
      loaders: ["neoforge"],
      beta: false,
      recommended: true,
      published: new Date(now - 2 * 24 * HOUR).toISOString(),
    },
    {
      id: "pack-ver-prev",
      name: "1.3.2",
      versionNumber: "1.3.2",
      gameVersion: "1.21.1",
      loaders: ["neoforge"],
      beta: false,
      recommended: false,
      published: new Date(now - 14 * 24 * HOUR).toISOString(),
    },
  ],
  modpack_readiness: () => ({
    javaMajor: 21,
    hasJava: true,
    recommendedMemoryMb: 4096,
  }),
  modpack_install: (args) =>
    fakeJob(`pack:${String(args.projectId)}`, `安装整合包 ${String(args.title)}`),
  modpack_import: (args) => fakeJob(`import:${String(args.token)}`, "导入本地整合包"),
};

let callbackId = 1;
// 事件：记下订阅，`emitPreview` 时调用（模拟后端推事件）
const callbacks = new Map<number, (message: unknown) => void>();
const listeners = new Map<string, number[]>();
function emitPreview(event: string, payload: unknown) {
  for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload });
}
// 和 @tauri-apps/api 约定的全局对象：invoke 走这里，事件订阅返回假编号
Object.assign(window, {
  __previewDrop: (which?: string) => {
    // `__previewDrop("skin")`：模拟旧版本报告的皮肤拖放。
    if (which === "skin") {
      emitPreview("files-dropped", {
        items: [
          {
            token: "skin-drop",
            name: "我的皮肤.png",
            kind: "skin",
            problem: null,
            title: null,
            version: null,
            icon: null,
            size: 2100,
            from: null,
            folder: null,
          },
        ],
        rests: [],
      });
      return;
    }
    if (which === "bundle") {
      emitPreview("files-reading", true);
      window.setTimeout(
        () =>
          emitPreview("files-dropped", {
            items: droppedBundle,
            rests: [{ name: bundleName, count: 12 }],
          }),
        1000,
      );
      return;
    }
    emitPreview("files-dropped", {
      items:
        which === "pack" ? droppedPack : which === "minecraft" ? droppedGameRoot : droppedSample,
      rests: [],
    });
  },
  __previewHover: (on: boolean) => emitPreview("file-drag-hover", on),
  // 大世界要不要自动备份：控制台里 `__previewBackupAsk()`
  __previewBackupAsk: () =>
    emitPreview("backup-ask", {
      instanceId: "验收测试",
      folder: "新的世界",
      worldName: "新的世界",
      bytes: 1_520_000_000,
    }),
  __previewMemory: (mb: number) => {
    availableMb = mb;
  },
  __previewEmit: emitPreview,
  __previewAuthError: (code: string | null) => {
    previewAuthError = code;
  },
  __TAURI_INTERNALS__: {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
    transformCallback: (callback: (message: unknown) => void) => {
      const id = callbackId++;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback: () => undefined,
    convertFileSrc: (path: string) => path,
    invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
      if (cmd === "plugin:event|listen") {
        const event = String(args.event);
        const handler = Number(args.handler);
        listeners.set(event, [...(listeners.get(event) ?? []), handler]);
        return handler;
      }
      if (cmd.startsWith("plugin:event|")) return callbackId++;
      if (cmd.startsWith("plugin:")) return null;
      const handler = handlers[cmd];
      if (!handler) {
        console.warn("预览页没有模拟这个命令", cmd, args);
        return null;
      }
      return handler(args);
    },
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => undefined },
});

// 模拟好后端再加载真正的界面
await import("../main");
