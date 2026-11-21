import { getCurrentWindow } from "@tauri-apps/api/window";
import { Check, ChevronLeft, ChevronRight, Minus, Palette, Search, Square, X } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useNavigate, useNavigationType } from "react-router";
import { reportError } from "@/lib/report";
import { ONBOARDING_FLOW, ONBOARDING_STEPS, useAccountStore } from "@/stores/accounts";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { AccountMenu } from "./account/AccountMenu";
import { LumiIcon, type LumiIconName } from "./art/LumiIcon";
import { TaskCenter } from "./TaskCenter";
import { Tooltip } from "./ui/Tooltip";

/** 标题栏右边的圆形图标按钮（搜索、设置……）：和全站的图标按钮一个样子，提示用我们自己的 */
export function TitleIconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Tooltip label={label} side="bottom">
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className="relative grid size-8 place-items-center rounded-icon text-fg-muted transition-colors duration-(--motion-press) hover:bg-surface-hover hover:text-fg"
      >
        {children}
      </button>
    </Tooltip>
  );
}

/**
 * 最小化、最大化、关闭：点得到的范围照 Windows 的习惯是整条高度的方块（窗口最大化时鼠标甩到右上角就能点到关闭），
 * 看得见的悬停和左边的图标按钮一样是个圆；关闭悬停是红色的圆
 */
function WindowButton({
  label,
  danger = false,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  const hover = danger
    ? "group-hover:bg-danger group-hover:text-on-danger"
    : "group-hover:bg-surface-hover group-hover:text-fg";
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className="group grid h-full w-11 place-items-center text-fg-muted"
    >
      <span
        className={`grid size-8 place-items-center rounded-icon transition-colors duration-(--motion-press) ${hover}`}
      >
        {children}
      </span>
    </button>
  );
}

interface Crumb {
  label: string;
  to?: string;
  /** 第一级前面的小图标（和侧栏那一项的 LumiIcon 保持一致） */
  icon?: LumiIconName;
}

/** 「发现内容」里能选的几类（地址里的 `kind`） */
const DISCOVER_KINDS = ["modpack", "mod", "resourcePack", "shader", "map", "server", "favorites"];

/** 现在在哪：按地址拼出「探索模组 › Sodium」「实例库 › 验收测试 › 添加内容」 */
function useCrumbs(): Crumb[] {
  const { t } = useTranslation();
  const location = useLocation();
  const instances = useGameStore((s) => s.instances);
  const pageTitle = useAppStore((s) => s.pageTitle);
  const parts = location.pathname.split("/").filter(Boolean).map(decodeURIComponent);
  const params = new URLSearchParams(location.search);
  const nameOf = (id: string) => instances.find((i) => i.id === id)?.name ?? id;
  const library: Crumb = { label: t("nav.instances"), to: "/instances", icon: "library" };

  if (parts[0] === "instances") {
    const crumbs: Crumb[] = [library];
    if (parts[1]) crumbs.push({ label: nameOf(parts[1]) });
    return crumbs;
  }
  if (parts[0] === "discover") {
    const instanceId = params.get("instance");
    const fromInstance = params.get("from") === "instance" && instanceId;
    // 从实例内容列表点名字进来的：实例库 › 实例名 › 项目
    const fromContent = params.get("from") === "content" && instanceId;
    // 第一级跟着下面选的那一类变：探索整合包、探索模组……收藏夹就叫收藏夹
    const wanted = params.get("fromKind") === "favorites" ? "favorites" : params.get("kind");
    const kind = wanted && DISCOVER_KINDS.includes(wanted) ? wanted : "mod";
    const explore =
      kind === "favorites"
        ? t("kind.favorites")
        : t("discover.explore", { kind: t(`kind.${kind}`) });
    // 点回列表：从收藏夹点进详情的，回收藏夹（其他条件原样带回去，回来时筛选还在）
    const listParams = new URLSearchParams(params);
    if (listParams.get("fromKind") === "favorites") listParams.set("kind", "favorites");
    listParams.delete("fromKind");
    const listUrl = parts[1] ? `/discover?${listParams.toString()}` : undefined;
    const crumbs: Crumb[] = fromContent
      ? [library, { label: nameOf(instanceId), to: `/instances/${encodeURIComponent(instanceId)}` }]
      : fromInstance
        ? [
            library,
            { label: nameOf(instanceId), to: `/instances/${encodeURIComponent(instanceId)}` },
            { label: t("detail.add"), to: listUrl },
          ]
        : [{ label: explore, icon: "discover", to: listUrl }];
    if (parts[1]) {
      const title = (location.state as { entry?: { hit: { title: string } } } | null)?.entry?.hit
        .title;
      crumbs.push({ label: title ?? pageTitle ?? t("titlebar.project") });
    }
    return crumbs;
  }
  if (parts[0] === "dev") return [{ label: "组件展示页（开发版）" }];
  return [];
}

/**
 * 后退 / 前进：用路由自己记的历史位置判断能不能退、能不能进。
 * 新开一页（PUSH）时，后面的「前进」历史就作废了。
 */
function useHistoryButtons() {
  const location = useLocation();
  const type = useNavigationType();
  const furthest = useRef(0);
  const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
  useEffect(() => {
    furthest.current = type === "PUSH" ? idx : Math.max(furthest.current, idx);
  }, [location.key, type, idx]);
  return { canBack: idx > 0, canForward: idx < furthest.current };
}

/**
 * 鼠标侧键、Alt + ← / → 后退前进（Windows 上的习惯）。开着弹窗时不动，免得弹窗留在别的页上。
 * 侧键按下、松开都拦住，不让 WebView 自己再退一次。
 */
function useHistoryKeys(canBack: boolean, canForward: boolean) {
  const navigate = useNavigate();
  const can = useRef({ canBack, canForward });
  useEffect(() => {
    can.current = { canBack, canForward };
  }, [canBack, canForward]);
  useEffect(() => {
    const go = (delta: -1 | 1) => {
      if (document.querySelector('[role="dialog"]')) return;
      if (delta < 0 ? !can.current.canBack : !can.current.canForward) return;
      navigate(delta);
    };
    const onMouse = (event: MouseEvent) => {
      if (event.button !== 3 && event.button !== 4) return;
      event.preventDefault();
      if (event.type === "mouseup") go(event.button === 3 ? -1 : 1);
    };
    const onKey = (event: KeyboardEvent) => {
      if (!event.altKey || event.ctrlKey || event.shiftKey || event.metaKey) return;
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      go(event.key === "ArrowLeft" ? -1 : 1);
    };
    window.addEventListener("mousedown", onMouse);
    window.addEventListener("mouseup", onMouse);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onMouse);
      window.removeEventListener("mouseup", onMouse);
      window.removeEventListener("keydown", onKey);
    };
  }, [navigate]);
}

/** 后退、前进：和右边的图标按钮一样大（32 像素）；不带提示小框（贴着窗口左边，会被挡住） */
function HistoryButton({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="grid size-8 shrink-0 place-items-center rounded-icon text-fg-muted transition-colors duration-(--motion-press) hover:bg-surface-hover hover:text-fg disabled:opacity-35 disabled:hover:bg-transparent disabled:hover:text-fg-muted"
    >
      {children}
    </button>
  );
}

/**
 * 自定义标题栏：整条可拖动，双击最大化。本身没有底色：首页铺满截图时，截图顶上那层渐变就是窗口底色，
 * 所以在哪一页字的颜色都一样，切页面时不会跳色。
 * 左边：每一页都是 Lumi（点了回首页）+ 后退 / 前进（鼠标侧键、Alt + ← / → 也行）+ 现在在哪（点前面的一级能跳回去）。
 * 右边：账号、搜索、任务中心、设置是圆形图标按钮；最后是窗口的三个按钮。
 */
/**
 * 首次运行引导的步骤胶囊：在标题栏正中间。现在这一步是主色浅底；走过的打勾，能点回去
 */
function OnboardingSteps() {
  const { t } = useTranslation();
  const step = useAccountStore((s) => s.onboardingStep);
  const reached = useAccountStore((s) => s.onboardingReached);
  const findBlocked = useAccountStore((s) => s.onboardingFindBlocked);
  const setStep = useAccountStore((s) => s.setOnboardingStep);
  // 欢迎页排在四步后面：四个胶囊都打勾
  const index = ONBOARDING_FLOW.indexOf(step);
  return (
    <ol
      aria-label={t("onboarding.steps")}
      className="-translate-x-1/2 -translate-y-1/2 absolute top-1/2 left-1/2 flex items-center gap-1"
    >
      {ONBOARDING_STEPS.map((item, i) => {
        const current = i === index;
        const done = i < index;
        const canGo = i <= reached && !current && !findBlocked;
        return (
          <li key={item}>
            <button
              type="button"
              disabled={!canGo}
              aria-current={current ? "step" : undefined}
              onClick={() => setStep(item)}
              className={`flex h-7 items-center gap-1.5 rounded-control px-3 text-sm transition-colors duration-(--motion-press) disabled:cursor-default ${
                current
                  ? "bg-accent-soft font-medium text-accent"
                  : done
                    ? "text-accent hover:bg-surface-hover"
                    : "text-fg-muted enabled:hover:bg-surface-hover"
              }`}
            >
              {done ? (
                <Check className="size-3.5" aria-hidden />
              ) : (
                <span
                  aria-hidden
                  className={`size-1.5 rounded-icon ${current ? "bg-accent" : "bg-fg-muted/60"}`}
                />
              )}
              {t(`onboarding.step.${item}`)}
            </button>
          </li>
        );
      })}
    </ol>
  );
}

export function TitleBar() {
  const { t } = useTranslation();
  const onboarding = useAccountStore((s) => s.onboarding);
  const onboardingStep = useAccountStore((s) => s.onboardingStep);
  const appWindow = getCurrentWindow();
  const openSettings = useAppStore((s) => s.openSettings);
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  const navigate = useNavigate();
  const crumbs = useCrumbs();
  const location = useLocation();
  const { canBack, canForward } = useHistoryButtons();
  useHistoryKeys(canBack, canForward);
  const run = (action: () => Promise<void>, context: string) => () => {
    action().catch((error: unknown) => reportError(error, context));
  };

  return (
    <header
      data-tauri-drag-region
      className="relative flex h-11 shrink-0 items-center justify-between gap-4"
    >
      <nav
        data-tauri-drag-region
        aria-label={t("titlebar.where")}
        className="flex min-w-0 items-center gap-1 pl-3.5 text-sm"
      >
        {/* 每一页最左边都是 Lumi，点它回首页（在首页、首次运行引导里只是名字） */}
        {location.pathname === "/" || onboarding ? (
          <span data-tauri-drag-region className="shrink-0 px-3 font-semibold text-fg text-sm">
            {t("app.name")}
          </span>
        ) : (
          <button
            type="button"
            aria-label={t("titlebar.home")}
            onClick={() => navigate("/")}
            className="h-8 shrink-0 rounded-control px-3 font-semibold text-fg text-sm transition-colors duration-(--motion-press) hover:bg-surface-hover"
          >
            {t("app.name")}
          </button>
        )}
        {!onboarding && (
          <>
            <HistoryButton
              label={t("titlebar.back")}
              disabled={!canBack}
              onClick={() => navigate(-1)}
            >
              <ChevronLeft className="size-[18px]" aria-hidden />
            </HistoryButton>
            <HistoryButton
              label={t("titlebar.forward")}
              disabled={!canForward}
              onClick={() => navigate(1)}
            >
              <ChevronRight className="size-[18px]" aria-hidden />
            </HistoryButton>
            {crumbs.length > 0 && (
              <ol className="ml-1 flex min-w-0 items-center gap-1">
                {crumbs.map((crumb, i) => {
                  const last = i === crumbs.length - 1;
                  const content = (
                    <>
                      {crumb.icon && <LumiIcon name={crumb.icon} className="size-4 shrink-0" />}
                      <span className="min-w-0 truncate">{crumb.label}</span>
                    </>
                  );
                  return (
                    <li key={`${i}-${crumb.label}`} className="flex min-w-0 items-center gap-1">
                      {i > 0 && <span className="shrink-0 text-fg-muted/60">›</span>}
                      {crumb.to && !last ? (
                        <button
                          type="button"
                          onClick={() => crumb.to && navigate(crumb.to)}
                          className="flex min-w-0 items-center gap-1.5 rounded-control px-1.5 py-0.5 text-fg-muted transition-colors duration-(--motion-press) hover:bg-surface-hover hover:text-fg"
                        >
                          {content}
                        </button>
                      ) : (
                        <span
                          data-tauri-drag-region
                          className={`flex min-w-0 items-center gap-1.5 px-1.5 ${last ? "font-medium text-fg" : "text-fg-muted"}`}
                        >
                          {content}
                        </span>
                      )}
                    </li>
                  );
                })}
              </ol>
            )}
          </>
        )}
      </nav>
      {onboarding && onboardingStep !== "welcome" && <OnboardingSteps />}
      <div className="flex h-full shrink-0 items-center">
        {/* 首次运行引导里右边只留窗口的三个按钮：账号在第二步管，别的都用不上，中间留给步骤胶囊 */}
        {!onboarding && (
          <div className="flex items-center gap-1 pr-2">
            <AccountMenu />
            {/* 开发版才有：组件展示页的入口 */}
            {import.meta.env.DEV && (
              <TitleIconButton
                label="组件展示页（开发版）"
                onClick={() => navigate("/dev/components")}
              >
                <Palette className="size-4" aria-hidden />
              </TitleIconButton>
            )}
            <TitleIconButton label={t("titlebar.search")} onClick={() => setPaletteOpen(true)}>
              <Search className="size-4" aria-hidden />
            </TitleIconButton>
            <TaskCenter />
            <TitleIconButton label={t("titlebar.settings")} onClick={() => openSettings()}>
              <LumiIcon name="settings" className="size-4" />
            </TitleIconButton>
          </div>
        )}
        <WindowButton
          label={t("titlebar.minimize")}
          onClick={run(() => appWindow.minimize(), "最小化窗口")}
        >
          <Minus className="size-4" aria-hidden />
        </WindowButton>
        <WindowButton
          label={t("titlebar.maximize")}
          onClick={run(() => appWindow.toggleMaximize(), "最大化窗口")}
        >
          <Square className="size-3.5" aria-hidden />
        </WindowButton>
        <WindowButton
          danger
          label={t("titlebar.close")}
          onClick={run(() => appWindow.close(), "关闭窗口")}
        >
          <X className="size-4" aria-hidden />
        </WindowButton>
      </div>
    </header>
  );
}
