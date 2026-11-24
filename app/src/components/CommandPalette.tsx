import {
  Coffee,
  CornerDownLeft,
  Download,
  HardDrive,
  Heart,
  History,
  Info,
  type LucideIcon,
  Package,
  Palette,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { LumiIcon } from "@/components/art/LumiIcon";
import { InstanceThumb } from "@/components/discover/TargetPicker";
import { instanceSubtitle } from "@/components/InstanceCard";
import { duration, ease, spring } from "@/lib/motion";
import { type SettingsSection, useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";

type Group = "worlds" | "instances" | "pages" | "actions" | "search";
const GROUPS: Group[] = ["worlds", "instances", "pages", "actions", "search"];

interface Command {
  id: string;
  group: Group;
  title: string;
  subtitle?: string;
  /** 额外能搜到它的词（拼音首字母、英文名之类） */
  keywords?: string;
  icon: ReactNode;
  run: () => void;
}

/** 一组最多显示几条：总数不超过 30，不用虚拟滚动（docs/DESIGN.md 第 7 节） */
const PER_GROUP = 6;

const iconOf = (Icon: LucideIcon) => <Icon className="size-4" aria-hidden />;

/**
 * 命令面板（Ctrl + K，或者标题栏的放大镜）：打几个字就能到任何地方。
 * 继续玩最近的世界、打开实例、去各个页面、打开设置的某一类、新建实例；
 * 打了字还能直接带着这个词去资源中心搜。上下键选，回车执行，Esc 关掉。
 */
export function CommandPalette() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = useAppStore((s) => s.paletteOpen);
  const setOpen = useAppStore((s) => s.setPaletteOpen);
  const openSettings = useAppStore((s) => s.openSettings);
  const instances = useGameStore((s) => s.instances);
  const recent = useGameStore((s) => s.recentWorlds);
  const launch = useGameStore((s) => s.launch);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      // 等弹层挂上再聚焦
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const close = () => setOpen(false);

  const commands: Command[] = useMemo(() => {
    const worldOf = new Map((recent ?? []).map((r) => [r.instanceId, r.world]));
    const go = (path: string) => () => navigate(path);
    const settings = (section: SettingsSection) => () => openSettings(section);
    const q = query.trim();
    const list: Command[] = [
      ...(recent ?? [])
        .filter((r) => r.quickPlay)
        .map((r) => {
          const instance = instances.find((i) => i.id === r.instanceId);
          return {
            id: `world:${r.instanceId}`,
            group: "worlds" as const,
            title: t("palette.resume", { world: r.world.name }),
            subtitle: instance?.name,
            keywords: `${r.world.name} ${instance?.name ?? ""}`,
            icon: iconOf(Play),
            run: () => void launch(r.instanceId, r.world.folder),
          };
        }),
      ...instances.map((instance) => ({
        id: `instance:${instance.id}`,
        group: "instances" as const,
        title: instance.name,
        subtitle: instanceSubtitle(instance),
        keywords: instance.pack ?? "",
        icon: (
          <InstanceThumb
            instance={instance}
            world={worldOf.get(instance.id)}
            className="h-6 w-9 rounded-row"
          />
        ),
        run: go(`/instances/${encodeURIComponent(instance.id)}`),
      })),
      {
        id: "page:home",
        group: "pages",
        title: t("nav.home"),
        keywords: "home shouye",
        icon: <LumiIcon name="home" className="size-4" />,
        run: go("/"),
      },
      {
        id: "page:instances",
        group: "pages",
        title: t("nav.instances"),
        keywords: "instances shili",
        icon: <LumiIcon name="library" className="size-4" />,
        run: go("/instances"),
      },
      {
        id: "page:discover",
        group: "pages",
        title: t("discover.title"),
        keywords: "discover ziyuan mod",
        icon: <LumiIcon name="discover" className="size-4" />,
        run: go("/discover"),
      },
      {
        id: "page:favorites",
        group: "pages",
        title: t("kind.favorites"),
        keywords: "favorites shoucang",
        icon: iconOf(Heart),
        run: go("/discover?kind=favorites"),
      },
      {
        id: "settings:appearance",
        group: "pages",
        title: t("palette.settings", { section: t("settings.appearance") }),
        keywords:
          "settings theme shezhi waiguan 深色 浅色 主题 主色 自动封面 流畅 精美 动效 音效 sound language 语言",
        icon: iconOf(Palette),
        run: settings("appearance"),
      },
      {
        id: "settings:game",
        group: "pages",
        title: t("palette.settings", { section: t("settings.game") }),
        keywords:
          "settings memory ram gpu maximized shezhi youxi 内存 整理内存 内存优化 显卡 窗口 最大化 游戏语言 最小化 释放界面 聚焦 关联目录 高级 钩子 启动前 退出后 标题 discord",
        icon: iconOf(Settings),
        run: settings("game"),
      },
      {
        id: "settings:sync",
        group: "pages",
        title: t("palette.settings", { section: t("settings.sync") }),
        keywords:
          "settings sync instance options servers packs command hotbar shezhi tongbu 实例同步 跨实例 同步 按键 服务器 通用资源包 光影 命令记录 创造 物品栏",
        icon: iconOf(RefreshCw),
        run: settings("sync"),
      },
      {
        id: "settings:java",
        group: "pages",
        title: t("palette.settings", { section: t("settings.java") }),
        keywords: "settings java jvm gc args shezhi 运行时 参数 垃圾回收 环境变量",
        icon: iconOf(Coffee),
        run: settings("java"),
      },
      {
        id: "settings:worlds",
        group: "pages",
        title: t("palette.settings", { section: t("settings.worlds") }),
        keywords:
          "settings worlds storage backup cache shezhi shijie cunchu 存储 存档 自动备份 备份 清理 缓存 游戏目录 闲置版本 导出设置 导入设置",
        icon: iconOf(HardDrive),
        run: settings("worlds"),
      },
      {
        id: "settings:download",
        group: "pages",
        title: t("palette.settings", { section: t("settings.download") }),
        keywords:
          "settings download doh proxy shezhi xiazai 下载 代理 并发 连通性 网络诊断 补件 前置 平台",
        icon: iconOf(Download),
        run: settings("download"),
      },
      {
        id: "page:downloads",
        group: "pages",
        title: t("downloads.title"),
        keywords: "download history xiazai lishi renwu 任务中心 下载历史",
        icon: iconOf(History),
        run: () => useGameStore.getState().setHistoryOpen(true),
      },
      {
        id: "settings:about",
        group: "pages",
        title: t("palette.settings", { section: t("settings.about") }),
        keywords:
          "about version update onboarding license privacy credits thirdparty kokoa kokoa77 author developer guanyu 更新 检查更新 引导 版本 开源 许可 隐私 致谢 第三方 开发者 作者",
        icon: iconOf(Info),
        run: settings("about"),
      },
      {
        id: "action:new",
        group: "actions",
        title: t("nav.newInstance"),
        keywords: "new create xinjian",
        icon: iconOf(Plus),
        run: go("/instances?new=1"),
      },
      ...(q
        ? [
            {
              id: "search:mod",
              group: "search" as const,
              title: t("palette.searchIn", { q, kind: t("kind.mod") }),
              icon: iconOf(Search),
              run: go(`/discover?kind=mod&q=${encodeURIComponent(q)}`),
            },
            {
              id: "search:modpack",
              group: "search" as const,
              title: t("palette.searchIn", { q, kind: t("kind.modpack") }),
              icon: iconOf(Package),
              run: go(`/discover?kind=modpack&q=${encodeURIComponent(q)}`),
            },
          ]
        : []),
    ];
    const needle = q.toLowerCase();
    const matched = needle
      ? list.filter(
          (c) =>
            c.group === "search" ||
            `${c.title} ${c.subtitle ?? ""} ${c.keywords ?? ""}`.toLowerCase().includes(needle),
        )
      : list;
    // 按组排，每组最多几条
    return GROUPS.flatMap((g) => matched.filter((c) => c.group === g).slice(0, PER_GROUP));
  }, [query, instances, recent, t, navigate, openSettings, launch]);

  useEffect(() => {
    setActive((i) => Math.min(i, Math.max(0, commands.length - 1)));
  }, [commands.length]);

  // 选中项跟着键盘滚到看得见的地方
  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const runAt = (index: number) => {
    const command = commands[index];
    if (!command) return;
    close();
    command.run();
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((i) => (commands.length === 0 ? 0 : (i + 1) % commands.length));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((i) => (commands.length === 0 ? 0 : (i - 1 + commands.length) % commands.length));
    } else if (event.key === "Enter") {
      event.preventDefault();
      runAt(active);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  };

  let lastGroup: Group | null = null;

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-50 flex justify-center bg-scrim pt-[14vh]"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1, transition: { duration: duration.overlay, ease: ease.out } }}
          exit={{ opacity: 0, transition: { duration: duration.press, ease: ease.in } }}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) close();
          }}
        >
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label={t("palette.title")}
            className="flex h-fit max-h-[70vh] w-[640px] max-w-[calc(100vw-48px)] flex-col overflow-hidden rounded-dialog border border-line bg-overlay shadow-dialog"
            initial={{ opacity: 0, scale: 0.98, y: -8 }}
            animate={{ opacity: 1, scale: 1, y: 0, transition: spring.snappy }}
            exit={{
              opacity: 0,
              scale: 0.99,
              transition: { duration: duration.press, ease: ease.in },
            }}
            onKeyDown={onKeyDown}
          >
            <div className="flex items-center gap-3 border-line border-b px-5">
              <Search className="size-5 shrink-0 text-fg-muted" aria-hidden />
              <input
                ref={inputRef}
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActive(0);
                }}
                placeholder={t("palette.placeholder")}
                aria-label={t("palette.placeholder")}
                className="h-14 min-w-0 flex-1 bg-transparent text-base text-fg outline-none"
              />
            </div>
            <div ref={listRef} role="listbox" className="min-h-0 flex-1 overflow-y-auto p-2">
              {commands.length === 0 && (
                <p className="px-3 py-8 text-center text-fg-muted text-sm">{t("palette.empty")}</p>
              )}
              {commands.map((command, index) => {
                const header = command.group !== lastGroup;
                lastGroup = command.group;
                return (
                  <div key={command.id}>
                    {header && (
                      <p className="px-3 pt-2 pb-1 text-fg-muted text-xs">
                        {t(`palette.group.${command.group}`)}
                      </p>
                    )}
                    <button
                      type="button"
                      role="option"
                      aria-selected={index === active}
                      data-index={index}
                      onMouseMove={() => setActive(index)}
                      onClick={() => runAt(index)}
                      className={`relative flex w-full items-center gap-3 rounded-item px-3 py-2 text-left text-fg text-sm transition-colors duration-(--motion-press) ${
                        index === active ? "bg-surface-hover" : ""
                      }`}
                    >
                      {index === active && (
                        <span
                          className="absolute top-2 bottom-2 left-0 w-[3px] rounded-tag bg-accent"
                          aria-hidden
                        />
                      )}
                      <span
                        className={`grid w-9 shrink-0 place-items-center ${index === active ? "text-accent" : "text-fg-muted"}`}
                      >
                        {command.icon}
                      </span>
                      <span className="min-w-0 flex-1 truncate">{command.title}</span>
                      {command.subtitle && (
                        <span className="max-w-[40%] shrink-0 truncate text-fg-muted text-xs">
                          {command.subtitle}
                        </span>
                      )}
                      {index === active && (
                        <CornerDownLeft className="size-3.5 shrink-0 text-accent" aria-hidden />
                      )}
                    </button>
                  </div>
                );
              })}
            </div>
            <div className="flex gap-4 border-line border-t px-5 py-2.5 text-fg-muted text-xs">
              <span>{t("palette.hint.move")}</span>
              <span>{t("palette.hint.run")}</span>
              <span>{t("palette.hint.close")}</span>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
