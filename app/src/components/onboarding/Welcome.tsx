import { ArrowRight } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import type { RecentWorld } from "@/lib/bindings";
import { duration, ease, spring } from "@/lib/motion";
import { useAccountStore } from "@/stores/accounts";
import { useGameStore } from "@/stores/game";
import { LightField } from "../art/LightField";
import { Button } from "../ui/Button";
import { GlowOrb } from "./parts";
import type { ReadyChoice } from "./steps";

/**
 * 引导最后的整窗欢迎页：一团光绽开、光点飘起来，「一切就绪」。
 * 「开始使用」（或者回车）才真的去做上一步选的事：继续那个世界 / 新建 / 去找整合包 / 去实例库 / 回首页
 */
export function Welcome({ choice, world }: { choice: ReadyChoice; world: RecentWorld | null }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const launch = useGameStore((s) => s.launch);
  const finish = useAccountStore((s) => s.finishOnboarding);
  const startNew = useAccountStore((s) => s.setStartNew);
  const started = useRef(false);
  const quickWorld = world?.quickPlay ? world.world : null;

  const go = async () => {
    if (started.current) return;
    started.current = true;
    await finish();
    switch (choice) {
      case "continue":
        navigate("/");
        if (world) void launch(world.instanceId, quickWorld?.folder);
        break;
      case "new":
        navigate("/");
        startNew(true);
        break;
      case "discover":
        navigate("/discover?kind=modpack");
        break;
      case "library":
        navigate("/instances");
        break;
      default:
        navigate("/");
    }
  };

  // 回车也能开始。已聚焦的按钮仍按原本的键盘操作处理。
  const goRef = useRef(go);
  goRef.current = go;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Enter" || event.defaultPrevented) return;
      if (event.target instanceof HTMLElement && event.target.closest("button, a, input")) return;
      void goRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const action =
    choice === "continue" && world
      ? t(quickWorld ? "onboarding.welcome.continue" : "onboarding.welcome.startGame", {
          name: world.world.name,
        })
      : t(`onboarding.welcome.${choice === "continue" ? "later" : choice}`);

  return (
    // 点任意位置都能开始（标题栏不算，它在外面）；键盘用回车或者 Tab 到「开始使用」
    <div
      className="relative flex min-h-0 flex-1 cursor-pointer flex-col items-center justify-center overflow-hidden px-6 pb-16 text-center"
      onClick={() => {
        if (window.getSelection()?.isCollapsed !== false) void go();
      }}
    >
      <div
        aria-hidden
        className="absolute inset-x-0 bottom-0 h-2/3 bg-radial-[ellipse_at_50%_100%] from-glow to-transparent to-70%"
      />
      <LightField count={12} seed={23} className="absolute inset-0" />
      {/* 中间一团柔光（以后放应用图标）：慢慢亮起来，没有边框 */}
      <motion.div
        className="relative"
        initial={{ opacity: 0, scale: 0.85 }}
        animate={{ opacity: 1, scale: 1, transition: spring.gentle }}
      >
        <GlowOrb className="size-72" />
      </motion.div>
      <motion.div
        className="relative -mt-14 flex flex-col items-center gap-2"
        initial={{ opacity: 0, y: 12 }}
        animate={{
          opacity: 1,
          y: 0,
          transition: { duration: duration.enter, ease: ease.out, delay: 0.12 },
        }}
      >
        <h1 className="cursor-text select-text font-semibold text-hero leading-tight">
          {t("onboarding.welcome.title")}
        </h1>
        <p className="cursor-text select-text text-base text-fg-muted">
          {t("onboarding.welcome.hint", { action })}
        </p>
        <Button
          variant="primary"
          className="mt-6 h-11! gap-2! px-6! text-base!"
          onClick={(event) => {
            event.stopPropagation();
            void go();
          }}
        >
          {t("onboarding.welcome.start")}
          <ArrowRight className="size-4" aria-hidden />
        </Button>
      </motion.div>
    </div>
  );
}
