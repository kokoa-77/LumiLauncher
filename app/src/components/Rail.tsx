import { LayoutGroup, motion } from "motion/react";
import { lazy, type MouseEvent, Suspense, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router";
import { duration, ease, spring } from "@/lib/motion";
import { logError } from "@/lib/report";
import { LumiIcon } from "./art/LumiIcon";
import { Tooltip } from "./ui/Tooltip";

const NewInstanceDialog = lazy(async () => ({
  default: (await import("./NewInstanceDialog")).NewInstanceDialog,
}));

/** 侧栏本身没有底色：首页截图左边那层渐变就是窗口底色，所以图标在哪一页都是同一套颜色 */
const tone = (active: boolean) =>
  active ? "text-accent" : "text-fg-muted hover:bg-surface-hover hover:text-fg";

const iconMovement = {
  home: ["translateY(1px)", "translateY(-2px)", "translateY(0)"],
  library: ["rotate(0deg)", "rotate(-9deg)", "rotate(0deg)"],
  discover: ["rotate(0deg) scale(0.96)", "rotate(11deg) scale(1.05)", "rotate(0deg) scale(1)"],
};

function NavIcon({
  to,
  icon,
  label,
  end = false,
  preload,
}: {
  to: string;
  icon: keyof typeof iconMovement;
  label: string;
  end?: boolean;
  preload?: () => void;
}) {
  const glyph = useRef<HTMLSpanElement>(null);
  const movement = useRef<Animation | undefined>(undefined);
  useEffect(() => () => movement.current?.cancel(), []);

  function respondToClick(event: MouseEvent<HTMLAnchorElement>) {
    if (
      event.defaultPrevented ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    const { fx, reduceMotion } = document.documentElement.dataset;
    if (
      fx !== "rich" ||
      reduceMotion === "true" ||
      matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      return;
    }
    movement.current?.cancel();
    movement.current = glyph.current?.animate(
      iconMovement[icon].map((transform) => ({ transform })),
      { duration: duration.enter * 1000, easing: `cubic-bezier(${ease.out.join(",")})` },
    );
  }

  return (
    <Tooltip label={label}>
      <NavLink
        to={to}
        end={end}
        aria-label={label}
        onPointerEnter={preload}
        onFocus={preload}
        onClick={respondToClick}
        className={({ isActive, isPending }) =>
          `relative grid size-11 place-items-center rounded-icon transition-[background-color,color,scale] duration-(--motion-press) active:scale-95 ${isPending && !isActive ? "bg-surface-hover text-fg" : tone(isActive)}`
        }
      >
        {({ isActive }) => (
          <>
            {isActive && (
              <motion.span
                layoutId="rail-active"
                className="absolute inset-0 rounded-icon bg-accent-soft"
                transition={spring.gentle}
              />
            )}
            <span className="lumi-nav-hover relative flex">
              <span ref={glyph} className="lumi-nav-glyph flex">
                <LumiIcon
                  name={icon}
                  active={isActive}
                  className={`size-5 ${isActive ? "lumi-rail-active" : ""}`}
                />
              </span>
            </span>
          </>
        )}
      </NavLink>
    </Tooltip>
  );
}

function preloadInstances() {
  void import("@/pages/InstancesPage").catch((error: unknown) => logError(error, "预加载实例库"));
}

function preloadDiscover() {
  void import("@/pages/DiscoverPage").catch((error: unknown) => logError(error, "预加载发现内容"));
}

/** 窄图标栏：首页、实例、资源，最下面是新建实例。设置和账号在右上角（docs/ui-backlog.md）。 */
export function Rail() {
  const { t } = useTranslation();
  const [creating, setCreating] = useState(false);
  const [creatingLoaded, setCreatingLoaded] = useState(false);
  return (
    <LayoutGroup>
      <nav className="flex w-[72px] shrink-0 flex-col items-center gap-2 pt-2 pb-4">
        <NavIcon to="/" icon="home" label={t("nav.home")} end />
        <NavIcon
          to="/instances"
          icon="library"
          label={t("nav.instances")}
          preload={preloadInstances}
        />
        <NavIcon
          to="/discover"
          icon="discover"
          label={t("nav.discover")}
          preload={preloadDiscover}
        />
        <div className="mt-auto">
          <Tooltip label={t("nav.newInstance")}>
            <button
              type="button"
              aria-label={t("nav.newInstance")}
              onClick={() => {
                setCreatingLoaded(true);
                setCreating(true);
              }}
              className={`grid size-11 place-items-center rounded-icon transition-[background-color,color,scale] duration-(--motion-press) active:scale-95 ${tone(false)}`}
            >
              <LumiIcon name="add" className="size-5" />
            </button>
          </Tooltip>
        </div>
      </nav>
      {creatingLoaded && (
        <Suspense fallback={null}>
          <NewInstanceDialog open={creating} onClose={() => setCreating(false)} />
        </Suspense>
      )}
    </LayoutGroup>
  );
}
