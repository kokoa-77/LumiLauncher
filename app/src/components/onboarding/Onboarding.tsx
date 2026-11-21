import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { unwrap } from "@/lib/api";
import { commands, type RecentWorld } from "@/lib/bindings";
import { useOwnAccent } from "@/lib/cover";
import { duration, ease, spring } from "@/lib/motion";
import { logError } from "@/lib/report";
import { ONBOARDING_STEPS, useAccountStore } from "@/stores/accounts";
import { useGameStore } from "@/stores/game";
import { createFindSelection } from "./findGames";
import { LanguageStep } from "./LanguageStep";
import { Scene } from "./Scene";
import { AccountStep, FindStep, LookStep, type ReadyChoice, ReadyStep } from "./steps";
import { useFound } from "./useFound";
import { Welcome } from "./Welcome";

/**
 * 首次运行引导：只在全新安装时出现（设置「关于」里能重新走一遍），
 * 占住内容区，标题栏照常能拖、能关，标题栏正中间是步骤胶囊。
 * 一张「左边画面、右边操作」的大卡片浮在微光的夜空（浅色是白天）上：
 * 语言 → 登录 → 导入 → 外观 → 准备好了，最后是整窗的欢迎页。
 */
export function Onboarding() {
  const step = useAccountStore((s) => s.onboardingStep);
  const setStep = useAccountStore((s) => s.setOnboardingStep);
  const instances = useGameStore((s) => s.instances);
  const reached = useAccountStore((s) => s.onboardingReached);
  const { found, addFolder } = useFound(reached >= ONBOARDING_STEPS.indexOf("find"));
  const [findSelection, setFindSelection] = useState(createFindSelection);
  const [recent, setRecent] = useState<RecentWorld | null | undefined>(undefined);
  const [picked, setPicked] = useState<ReadyChoice | null>(null);
  // 这里没有世界：主色用自己选的
  useOwnAccent();

  // 到最后一步时看看有没有世界（也包括引导中刚接入的目录）。
  useEffect(() => {
    if (step !== "ready") return;
    let cancelled = false;
    setRecent(undefined);
    commands
      .worldsRecent()
      .then(unwrap)
      .then((list) => {
        if (!cancelled) setRecent(list[0] ?? null);
      })
      .catch((error: unknown) => {
        logError(error, "找最近玩的世界");
        if (!cancelled) setRecent(null);
      });
    return () => {
      cancelled = true;
    };
  }, [step]);

  const ready =
    recent === undefined
      ? undefined
      : recent && {
          world: recent,
          instance: instances.find((i) => i.id === recent.instanceId),
        };
  const choice = !picked || (picked === "continue" && !recent) ? "later" : picked;

  return (
    <AnimatePresence mode="wait" initial={false}>
      {step === "welcome" ? (
        <motion.div
          key="welcome"
          className="relative flex min-h-0 flex-1 flex-col"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1, transition: { duration: duration.page, ease: ease.out } }}
        >
          <Welcome choice={choice} world={recent ?? null} />
        </motion.div>
      ) : (
        <motion.div
          key="steps"
          className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden px-6 pt-1 pb-6"
          exit={{ opacity: 0, transition: { duration: duration.overlay, ease: ease.in } }}
        >
          <motion.div
            className="relative grid h-full max-h-[760px] w-full max-w-[1160px] overflow-hidden rounded-dialog border border-line bg-overlay shadow-dialog md:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]"
            initial={{ opacity: 0, y: 14, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1, transition: spring.gentle }}
          >
            <Scene step={step} found={found} ready={ready} />
            <section className="relative flex min-h-0 flex-col">
              <AnimatePresence mode="wait" initial={false}>
                <motion.div
                  key={step}
                  className="flex min-h-0 flex-1 flex-col"
                  initial={{ opacity: 0, x: 16 }}
                  animate={{
                    opacity: 1,
                    x: 0,
                    transition: { duration: duration.enter, ease: ease.out },
                  }}
                  exit={{
                    opacity: 0,
                    x: -12,
                    transition: { duration: duration.overlay, ease: ease.in },
                  }}
                >
                  {step === "language" && <LanguageStep onNext={() => setStep("account")} />}
                  {step === "account" && (
                    <AccountStep
                      onNext={() => setStep("find")}
                      onBack={() => setStep("language")}
                    />
                  )}
                  {step === "find" && (
                    <FindStep
                      found={found}
                      addFolder={addFolder}
                      selection={findSelection}
                      setSelection={setFindSelection}
                      onNext={() => setStep("look")}
                      onBack={() => setStep("account")}
                    />
                  )}
                  {step === "look" && (
                    <LookStep onNext={() => setStep("ready")} onBack={() => setStep("find")} />
                  )}
                  {step === "ready" && (
                    <ReadyStep
                      ready={ready}
                      choice={choice}
                      onChoose={setPicked}
                      onNext={() => setStep("welcome")}
                      onBack={() => setStep("look")}
                    />
                  )}
                </motion.div>
              </AnimatePresence>
            </section>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
