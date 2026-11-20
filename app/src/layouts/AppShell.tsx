import { AnimatePresence, LayoutGroup, motion, useReducedMotionConfig } from "motion/react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { useLocation, useOutlet } from "react-router";
import { LightField } from "@/components/art/LightField";
import { DropOverlay } from "@/components/DropOverlay";
import { HomeBackdrop } from "@/components/HomeBackdrop";
import { LaunchCapsule } from "@/components/LaunchCapsule";
import { Rail } from "@/components/Rail";
import { TitleBar } from "@/components/TitleBar";
import { duration, ease, pageVariants, routeDirection } from "@/lib/motion";
import { useAccountStore } from "@/stores/accounts";
import { useAppStore } from "@/stores/app";
import { useClaimStore } from "@/stores/claims";
import { useDropStore } from "@/stores/drops";
import { useFolderStore } from "@/stores/folders";
import { useGameStore } from "@/stores/game";

// 设置弹窗、命令面板、拖文件进来的弹窗第一次打开时才加载，不占首屏的 JS
const CommandPalette = lazy(async () => ({
  default: (await import("@/components/CommandPalette")).CommandPalette,
}));
const SettingsDialog = lazy(async () => ({
  default: (await import("@/components/SettingsDialog")).SettingsDialog,
}));
const DropDialog = lazy(async () => ({
  default: (await import("@/components/DropDialog")).DropDialog,
}));
// 大世界要不要自动备份（D17j）：第一次需要时才加载
const BackupAskDialog = lazy(async () => ({
  default: (await import("@/components/worlds/BackupAskDialog")).BackupAskDialog,
}));
// 「移到分组」（D17k）：第一次需要时才加载
const GroupDialog = lazy(async () => ({
  default: (await import("@/components/GroupDialog")).GroupDialog,
}));
// 启动前内存不够的提示（D17h）：第一次需要时才加载
const MemoryShortDialog = lazy(async () => ({
  default: (await import("@/components/MemoryShortDialog")).MemoryShortDialog,
}));
// 启动前极速预检提示（D25d）：第一次命中问题时才加载
const PreflightDialog = lazy(async () => ({
  default: (await import("@/components/PreflightDialog")).PreflightDialog,
}));
// 别家启动器的文件夹：「添加 .minecraft 文件夹」「从别的启动器导入」（D19）：第一次需要时才加载
const FolderDialogs = lazy(async () => ({
  default: (await import("@/components/FolderDialogs")).FolderDialogs,
}));
// 首次运行引导（D19b）：只有全新安装用得到，按需加载
const Onboarding = lazy(async () => ({
  default: (await import("@/components/onboarding/Onboarding")).Onboarding,
}));
// Microsoft 登录和皮肤预览：第一次需要时才加载
const AccountDialogs = lazy(async () => ({
  default: (await import("@/components/account/AccountDialogs")).AccountDialogs,
}));
// 下载历史（D18）：第一次点「查看下载历史」时才加载
const DownloadHistory = lazy(async () => ({
  default: (await import("@/components/downloads/DownloadHistory")).DownloadHistory,
}));

// 只有打开补件或实时日志时才取面板，之后保持挂载以保留退场动画。
const ClaimDialog = lazy(async () => ({
  default: (await import("@/components/ClaimDialog")).ClaimDialog,
}));
const CrashRollbackDialog = lazy(async () => ({
  default: (await import("@/components/CrashRollbackDialog")).CrashRollbackDialog,
}));
const LogDrawer = lazy(async () => ({
  default: (await import("@/components/LogDrawer")).LogDrawer,
}));
const NewInstanceDialog = lazy(async () => ({
  default: (await import("@/components/NewInstanceDialog")).NewInstanceDialog,
}));
const StopGameDialog = lazy(async () => ({
  default: (await import("@/components/StopGameDialog")).StopGameDialog,
}));

/**
 * 整体布局：标题栏 + 窄图标栏 + 内容区（页面切换带过渡）。
 * 骨架「首页沉浸」：首页将世界封面或默认插画铺满窗口，在标题栏和侧栏后面。
 * 离开首页恢复普通的实底布局（docs/ui-backlog.md）。
 *
 * 层次从下到上：窗口底色 → 首页截图（只在首页）→ 内容面板 → 卡片和控件。
 * 截图边上的渐变就是窗口底色，标题栏、侧栏在哪一页都是同一套颜色；
 * 离开首页时截图淡掉、内容面板同时浮上来，回到首页反过来，两边都用页面档的时长，看起来是一个动作。
 */
export function AppShell() {
  const location = useLocation();
  const outlet = useOutlet();
  const openSettings = useAppStore((s) => s.openSettings);
  const settingsOpen = useAppStore((s) => s.settingsSection !== null);
  const paletteOpen = useAppStore((s) => s.paletteOpen);
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  const backdrop = useAppStore((s) => s.backdrop);
  const reduceMotion = useReducedMotionConfig();
  const homeReady = useGameStore(
    (s) =>
      s.instancesLoaded &&
      (s.instances.length === 0 || (s.recentWorlds !== null && s.recentServers !== null)),
  );
  // 打开过一次就一直挂着，关闭时才有退场动画
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  useEffect(() => {
    if (settingsOpen) setSettingsLoaded(true);
  }, [settingsOpen]);
  const [paletteLoaded, setPaletteLoaded] = useState(false);
  useEffect(() => {
    if (paletteOpen) setPaletteLoaded(true);
  }, [paletteOpen]);
  const claimsOpen = useClaimStore((s) => s.shown !== null);
  const [claimsLoaded, setClaimsLoaded] = useState(false);
  useEffect(() => {
    if (claimsOpen) setClaimsLoaded(true);
  }, [claimsOpen]);
  const crashed = useGameStore((s) =>
    Object.values(s.sessions).some((session) => session.state.kind === "crashed"),
  );
  const [rollbackLoaded, setRollbackLoaded] = useState(false);
  useEffect(() => {
    if (crashed) setRollbackLoaded(true);
  }, [crashed]);
  const logOpen = useGameStore((s) => s.logInstance !== null);
  const [logLoaded, setLogLoaded] = useState(false);
  useEffect(() => {
    if (logOpen) setLogLoaded(true);
  }, [logOpen]);
  const dropOpen = useDropStore((s) => s.items !== null || s.pack !== null);
  const [dropLoaded, setDropLoaded] = useState(false);
  useEffect(() => {
    if (dropOpen) setDropLoaded(true);
  }, [dropOpen]);
  const memoryShort = useGameStore((s) => s.memoryShort !== null);
  const [memoryLoaded, setMemoryLoaded] = useState(false);
  useEffect(() => {
    if (memoryShort) setMemoryLoaded(true);
  }, [memoryShort]);
  const preflightAsk = useGameStore((s) => s.preflightAsk !== null);
  const [preflightLoaded, setPreflightLoaded] = useState(false);
  useEffect(() => {
    if (preflightAsk) setPreflightLoaded(true);
  }, [preflightAsk]);
  const groupAsk = useGameStore((s) => s.groupAsk !== null);
  const [groupLoaded, setGroupLoaded] = useState(false);
  useEffect(() => {
    if (groupAsk) setGroupLoaded(true);
  }, [groupAsk]);
  const historyOpen = useGameStore((s) => s.historyOpen);
  const [historyLoaded, setHistoryLoaded] = useState(false);
  useEffect(() => {
    if (historyOpen) setHistoryLoaded(true);
  }, [historyOpen]);
  const folderOpen = useFolderStore((s) => s.gameRoot !== null || s.foreign !== null);
  const [folderLoaded, setFolderLoaded] = useState(false);
  useEffect(() => {
    if (folderOpen) setFolderLoaded(true);
  }, [folderOpen]);
  const onboarding = useAccountStore((s) => s.onboarding);
  const immersive = location.pathname === "/" && !onboarding;
  const previousPath = useRef(location.pathname);
  const direction = routeDirection(previousPath.current, location.pathname);
  useEffect(() => {
    previousPath.current = location.pathname;
  }, [location.pathname]);
  const variants = pageVariants(reduceMotion);
  const startNew = useAccountStore((s) => s.startNew);
  const setStartNew = useAccountStore((s) => s.setStartNew);
  const [startNewLoaded, setStartNewLoaded] = useState(false);
  useEffect(() => {
    if (startNew) setStartNewLoaded(true);
  }, [startNew]);
  const stopAsk = useGameStore((s) => s.stopAsk !== null);
  const [stopAskLoaded, setStopAskLoaded] = useState(false);
  useEffect(() => {
    if (stopAsk) setStopAskLoaded(true);
  }, [stopAsk]);
  const accountOpen = useAccountStore((s) => s.addOpen || s.skinFor !== null);
  const [accountLoaded, setAccountLoaded] = useState(false);
  useEffect(() => {
    if (accountOpen) setAccountLoaded(true);
  }, [accountOpen]);
  const backupAsk = useGameStore((s) => s.backupAsk !== null);
  const [backupAskLoaded, setBackupAskLoaded] = useState(false);
  useEffect(() => {
    if (backupAsk) setBackupAskLoaded(true);
  }, [backupAsk]);

  // Ctrl + , 打开设置（Windows 软件的通用习惯）；Ctrl + K 打开 / 关上命令面板
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.key === ",") {
        event.preventDefault();
        openSettings();
      }
      if (event.ctrlKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen(!useAppStore.getState().paletteOpen);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openSettings, setPaletteOpen]);

  return (
    <div className="relative flex h-full flex-col">
      <AnimatePresence>
        {immersive && (
          <HomeBackdrop
            key={backdrop?.url ?? "home-default"}
            backdrop={backdrop}
            ready={homeReady}
          />
        )}
        {onboarding && (
          <motion.div
            key="onboarding-sky"
            aria-hidden
            className="absolute inset-0"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1, transition: { duration: duration.page, ease: ease.out } }}
            exit={{ opacity: 0, transition: { duration: duration.page, ease: ease.in } }}
          >
            <div className="absolute inset-0 bg-linear-to-b from-sky-top to-sky-bottom" />
            <LightField count={10} seed={11} className="absolute inset-0" />
          </motion.div>
        )}
      </AnimatePresence>
      <TitleBar />
      {onboarding ? (
        <div className="relative flex min-h-0 flex-1">
          <Suspense fallback={null}>
            <Onboarding />
          </Suspense>
        </div>
      ) : (
        <div className="relative flex min-h-0 flex-1">
          <Rail />
          <main className="relative min-w-0 flex-1 overflow-hidden">
            {/* 内容面板：到首页时往下沉一点并淡出，露出后面的截图；离开首页时浮上来 */}
            <div
              aria-hidden
              className={`absolute inset-0 rounded-tl-panel border-line border-t border-l bg-surface-2 transition-[opacity,translate] duration-(--motion-page) ease-in-out ${
                immersive ? "translate-y-3 opacity-0" : "translate-y-0 opacity-100"
              }`}
            />
            {/*
              使用 LayoutGroup + 无阻断 AnimatePresence（D25c）：
              新旧页面在同一帧并存（绝对定位叠放），使实例卡片 → 实例详情的 layoutId 共享封面变形真正衔接，
              且平级切页 / 下钻 / 返回各自具备空间方向反馈。
            */}
            <LayoutGroup id="app-pages">
              <AnimatePresence initial={false} custom={direction}>
                <motion.div
                  key={location.pathname}
                  custom={direction}
                  variants={variants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  className="absolute inset-0 overflow-y-auto"
                >
                  {outlet}
                </motion.div>
              </AnimatePresence>
            </LayoutGroup>
          </main>
        </div>
      )}
      {/* 引导最后点了「新建实例」：常驻在这里，弹窗里接着打开的导入、新建窗口不会跟着引导一起消失 */}
      {startNewLoaded && (
        <Suspense fallback={null}>
          <NewInstanceDialog open={startNew} onClose={() => setStartNew(false)} />
        </Suspense>
      )}
      <LaunchCapsule />
      <Suspense fallback={null}>
        {logLoaded && <LogDrawer />}
        {claimsLoaded && <ClaimDialog />}
        {rollbackLoaded && <CrashRollbackDialog />}
      </Suspense>
      {dropLoaded && (
        <Suspense fallback={null}>
          <DropDialog />
        </Suspense>
      )}
      <DropOverlay />
      {stopAskLoaded && (
        <Suspense fallback={null}>
          <StopGameDialog />
        </Suspense>
      )}
      {memoryLoaded && (
        <Suspense fallback={null}>
          <MemoryShortDialog />
        </Suspense>
      )}
      {preflightLoaded && (
        <Suspense fallback={null}>
          <PreflightDialog />
        </Suspense>
      )}
      {groupLoaded && (
        <Suspense fallback={null}>
          <GroupDialog />
        </Suspense>
      )}
      {backupAskLoaded && (
        <Suspense fallback={null}>
          <BackupAskDialog />
        </Suspense>
      )}
      {settingsLoaded && (
        <Suspense fallback={null}>
          <SettingsDialog />
        </Suspense>
      )}
      {folderLoaded && (
        <Suspense fallback={null}>
          <FolderDialogs />
        </Suspense>
      )}
      {accountLoaded && (
        <Suspense fallback={null}>
          <AccountDialogs />
        </Suspense>
      )}
      {historyLoaded && (
        <Suspense fallback={null}>
          <DownloadHistory />
        </Suspense>
      )}
      {paletteLoaded && (
        <Suspense fallback={null}>
          <CommandPalette />
        </Suspense>
      )}
    </div>
  );
}
