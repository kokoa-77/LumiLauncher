import { Languages } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";
import type { InstanceSummary, RecentWorld } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { duration, ease } from "@/lib/motion";
import { type OnboardingStep, useCurrentAccount } from "@/stores/accounts";
import { SkinPreview } from "../account/SkinPanel";
import { LightField } from "../art/LightField";
import { SceneBackground } from "../art/SceneBackground";
import { PlayerHead } from "../PlayerHead";
import { Tag } from "../ui/Tag";
import { SCENE_ART } from "./art";
import { MiniHome } from "./MiniHome";
import { GlowOrb } from "./parts";
import type { Found } from "./useFound";

/**
 * 引导左边的「画面」：天空（白天 / 夜晚跟着深浅色）+ 地平线的光雾 + 慢慢飘的光点，
 * 上面放这一步实时的东西——找到的游戏、你的 3D 人物、会变色的迷你首页、你的世界。
 * 插画（O1–O4）在天空和前景之间（`art.ts`）。
 */
export function Scene({
  step,
  found,
  ready,
}: {
  step: OnboardingStep;
  found: Found;
  ready: { world: RecentWorld; instance: InstanceSummary | undefined } | null | undefined;
}) {
  const art = SCENE_ART[step];
  return (
    <aside className="relative hidden overflow-hidden md:block">
      <div aria-hidden className="absolute inset-0 bg-linear-to-b from-sky-top to-sky-bottom" />
      <AnimatePresence initial={false}>
        {art && (
          <motion.div
            key={step}
            aria-hidden
            className="absolute inset-0"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1, transition: { duration: duration.page, ease: ease.out } }}
            exit={{ opacity: 0, transition: { duration: duration.page, ease: ease.in } }}
          >
            <SceneBackground art={art} />
            <div className="absolute inset-0 bg-linear-to-t from-hero-scrim/65 via-transparent to-transparent" />
          </motion.div>
        )}
      </AnimatePresence>
      {/* 地平线上的一层光雾（主色，换实例时一起变） */}
      <div
        aria-hidden
        className="absolute inset-x-0 bottom-0 h-2/3 bg-radial-[ellipse_at_50%_100%] from-glow to-transparent to-70%"
      />
      <LightField count={12} seed={3} area="low" className="absolute inset-0" />
      <AnimatePresence mode="wait" initial={false}>
        <motion.div
          key={step}
          className="absolute inset-0 flex items-center justify-center p-10"
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0, transition: { duration: duration.enter, ease: ease.out } }}
          exit={{ opacity: 0, y: -6, transition: { duration: duration.overlay, ease: ease.in } }}
        >
          {step === "language" && (
            <div aria-hidden className="relative grid place-items-center">
              <GlowOrb className="size-56" />
              <span className="absolute grid size-20 place-items-center rounded-panel border border-line bg-overlay text-accent shadow-overlay">
                <Languages className="size-9" />
              </span>
            </div>
          )}
          {step === "find" && <FoundStack found={found} />}
          {step === "account" && <Stage />}
          {step === "look" && <MiniHome />}
          {step === "ready" && <ReadyPicture ready={ready} />}
        </motion.div>
      </AnimatePresence>
    </aside>
  );
}

/** 找到的游戏一张张叠着（最多三张，后面的稍微歪一点） */
function FoundStack({ found }: { found: Found }) {
  const { t } = useTranslation();
  const cards = [
    ...[...found.linked, ...found.roots].map((root) => ({
      key: root.id,
      title: root.name,
      sub: t("linked.versions", { count: root.versions }),
      tag: t(`launchers.${root.launcher}`),
    })),
    ...found.foreign.map((item) => ({
      key: item.id,
      title: item.name,
      sub: [item.gameVersion, item.loader].filter(Boolean).join(" · "),
      tag: t(`launchers.${item.launcher}`),
    })),
  ];
  if (found.loading || cards.length === 0) {
    return (
      <div className="flex flex-col items-center gap-4 text-center">
        <GlowOrb />
        <p className="text-fg-muted text-sm">
          {found.loading ? t("onboarding.find.searching") : t("onboarding.find.nothing")}
        </p>
      </div>
    );
  }
  const tilt = ["-rotate-3", "rotate-2", "-rotate-1"];
  const shown = cards.slice(0, 3);
  return (
    <div className="relative flex w-full max-w-72 flex-col items-stretch">
      {shown.map((card, i) => (
        <div
          key={card.key}
          className={`relative -mt-3 first:mt-0 rounded-panel border border-line bg-overlay p-4 shadow-overlay ${tilt[i] ?? ""}`}
          style={{ zIndex: shown.length - i }}
        >
          <div className="flex items-center justify-between gap-2">
            <p className="truncate font-medium text-sm">{card.title}</p>
            <Tag className="text-2xs">{card.tag}</Tag>
          </div>
          <p className="mt-1 flex items-center gap-1.5 truncate text-fg-muted text-xs">
            <span aria-hidden className="size-1.5 shrink-0 rounded-icon bg-accent" />
            {card.sub}
          </p>
        </div>
      ))}
      {cards.length > shown.length && (
        <p className="mt-3 text-center text-fg-muted text-xs">
          {t("onboarding.find.more", { count: cards.length - shown.length })}
        </p>
      )}
    </div>
  );
}

/** 账号这一步：你的 3D 人物站在一束光里（还没账号时是一团光） */
function Stage() {
  const current = useCurrentAccount();
  return (
    <div className="relative flex flex-col items-center">
      <div
        aria-hidden
        className="-translate-x-1/2 absolute bottom-2 left-1/2 h-10 w-56 rounded-icon bg-radial from-glow to-transparent to-70%"
      />
      {current?.hasSkin ? (
        <SkinPreview account={current} width={260} height={380} className="bg-transparent" />
      ) : current ? (
        <div className="flex h-[380px] flex-col items-center justify-center gap-4">
          <GlowOrb className="size-40" />
          <PlayerHead name={current.name} className="-mt-24 size-14" />
        </div>
      ) : (
        <div className="flex h-[380px] items-center">
          <GlowOrb className="size-40" />
        </div>
      )}
    </div>
  );
}

/** 最后一步：你的世界（找到了的话），没有就是一团光 */
function ReadyPicture({
  ready,
}: {
  ready: { world: RecentWorld; instance: InstanceSummary | undefined } | null | undefined;
}) {
  const url = ready?.world.world.imageUrl;
  if (!ready || !url) return <GlowOrb className="size-44" />;
  // 只有 64×64 小图标的世界：小图标放在一团光里，不放大成马赛克
  if (ready.world.world.imageKind === "icon") {
    return (
      <div className="relative grid place-items-center">
        <GlowOrb className="size-56" />
        <img
          src={url}
          alt=""
          className="absolute size-20 rounded-row shadow-overlay [image-rendering:pixelated]"
        />
      </div>
    );
  }
  return (
    <div className="relative w-full max-w-80">
      <div
        aria-hidden
        className="-inset-10 absolute rounded-icon bg-radial from-glow to-transparent to-70%"
      />
      <div className="relative aspect-video overflow-hidden rounded-panel border border-line shadow-dialog">
        <img src={imageAt(url, 640)} alt="" className="size-full object-cover" />
      </div>
    </div>
  );
}
