// 组件展示页的单独入口：不用编译 Rust、不开启动器，`pnpm --dir app dev` 后浏览器打开
// http://localhost:1420/components.html 就能看。只在开发时存在，正式包不包含（vite 只打包 index.html）。
import "@/styles/global.css";
import "@/lib/i18n";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Toaster } from "sonner";
import { SettingsDialog } from "@/components/SettingsDialog";
import type { Settings } from "@/lib/bindings";
import { applyBackdrop, applyTheme, resolveTheme, systemPrefersDark } from "@/lib/theme";
import { useAppStore } from "@/stores/app";
import { useUpdateStore } from "@/stores/updates";
import { ComponentsPage } from "./ComponentsPage";

applyTheme(resolveTheme("system", systemPrefersDark()));
applyBackdrop("solid");

// 浏览器里没有后端：给设置弹窗一份假设置，改了只存在内存里
const settings: Settings = {
  schemaVersion: 2,
  appearance: {
    theme: "system",
    reduceMotion: false,
    effects: "auto",
    autoCover: true,
    accentFollowWorld: true,
    accentHue: 245,
    accentChroma: 0.15,
  },
  worlds: { autoBackup: false, keepBackups: 3, backupDir: null },
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
    envVars: [],
    hooks: { preLaunch: "", postExit: "" },
    customWindowTitle: "",
    discordRpc: false,
  },
};
useAppStore.setState({
  info: { version: "0.6.2（展示页）", backdrop: "solid", memoryMb: 32768 },
  settings,
  gameDir: { path: String.raw`C:\Games\Lumi\.minecraft`, locked: true },
  saveSettings: async (next) => useAppStore.setState({ settings: next }),
});
useUpdateStore.setState({ mode: "disabled" });

const container = document.getElementById("root");
if (!container) throw new Error("页面缺少 #root 容器");

createRoot(container).render(
  <StrictMode>
    <div className="h-full overflow-y-auto">
      <ComponentsPage />
    </div>
    <SettingsDialog />
    <Toaster position="bottom-right" />
  </StrictMode>,
);
