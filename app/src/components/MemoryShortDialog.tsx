import { Sparkles } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import { commands, type MemoryCheck, type ProgramMemory } from "@/lib/bindings";
import { gb } from "@/lib/memory";
import { logError } from "@/lib/report";
import { type MemoryShort, useGameStore } from "@/stores/game";
import { useMemoryStore } from "@/stores/memory";
import { MemoryBar } from "./MemoryBar";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";

/** 弹窗开着时多久重新看一次内存（用户可能正在关程序） */
const RECHECK_MS = 2000;
/** 「谁占着内存」多久重新列一次（遍历进程比读内存慢，隔久一点） */
const PROGRAMS_MS = 5000;

/**
 * 启动前检查（D17h）：点开始游戏时空闲内存不够，问用户怎么办。
 * 写清想要多少、现在空得出多少、关掉哪些程序能腾出多少；开着时每两秒重新看，用户关了程序、够了，
 * 主按钮就变成「开始游戏」。也能整理内存、这次少给一点、或者照样启动。
 */
export function MemoryShortDialog() {
  const { t } = useTranslation();
  const pending = useGameStore((s) => s.memoryShort);
  const dismiss = useGameStore((s) => s.dismissMemoryShort);
  const launch = useGameStore((s) => s.launch);
  const instances = useGameStore((s) => s.instances);
  const cleaning = useMemoryStore((s) => s.cleaning);
  const clean = useMemoryStore((s) => s.clean);
  const [check, setCheck] = useState<MemoryCheck | null>(null);
  const [programs, setPrograms] = useState<ProgramMemory[] | null>(null);
  // 最后一次打开时的那个请求（关掉再打开另一个实例时，旧的结果不要串过来）
  const openKey = useRef(0);
  // 关闭时还要画一会儿退场动画：内容用最后一次的
  const last = useRef<MemoryShort | null>(null);
  if (pending) last.current = pending;
  const view = pending ?? last.current;

  useEffect(() => {
    if (!pending) {
      setCheck(null);
      setPrograms(null);
      return;
    }
    const key = ++openKey.current;
    setCheck(pending.check);
    const recheck = async () => {
      try {
        const next = unwrap(await commands.memoryCheck(pending.instanceId));
        if (next && openKey.current === key) setCheck(next);
      } catch (error) {
        logError(error, "重新检查内存");
      }
    };
    const listPrograms = async () => {
      try {
        const next = await commands.memoryPrograms();
        if (openKey.current === key) setPrograms(next);
      } catch (error) {
        logError(error, "列出占内存的程序");
        if (openKey.current === key) setPrograms([]);
      }
    };
    void listPrograms();
    const checkTimer = window.setInterval(() => void recheck(), RECHECK_MS);
    const programsTimer = window.setInterval(() => void listPrograms(), PROGRAMS_MS);
    return () => {
      window.clearInterval(checkTimer);
      window.clearInterval(programsTimer);
    };
  }, [pending]);

  if (!view) return null;
  const current = check ?? view.check;
  const name = instances.find((i) => i.id === view.instanceId)?.name ?? view.instanceId;
  const shortMb = Math.max(0, current.wantedMb - current.spareMb);
  const smaller = !current.enough && current.fallbackMb < current.wantedMb;

  // 关掉前几个大的就够了：列到加起来够补上差额为止（至少列一个，最多列全部）
  const shown: ProgramMemory[] = [];
  let sum = 0;
  for (const program of programs ?? []) {
    if (shown.length > 0 && sum >= shortMb) break;
    shown.push(program);
    sum += program.memoryMb;
  }

  const go = (memoryMb?: number) =>
    void launch(view.instanceId, view.world, view.server, {
      skipCheck: true,
      memoryMb,
    });

  const cleanThenRecheck = async () => {
    const result = await clean();
    if (!result) return;
    try {
      const next = unwrap(await commands.memoryCheck(view.instanceId));
      if (next) setCheck(next);
      setPrograms(await commands.memoryPrograms());
    } catch (error) {
      logError(error, "整理后重新检查内存");
    }
  };

  const primary = current.enough ? (
    <Button variant="primary" onClick={() => go()}>
      {t("memory.short.play")}
    </Button>
  ) : current.auto ? (
    <Button variant="primary" onClick={() => go()}>
      {t("memory.short.playAuto", { gb: gb(current.fallbackMb) })}
    </Button>
  ) : smaller ? (
    <Button variant="primary" onClick={() => go(current.fallbackMb)}>
      {t("memory.short.playSmaller", { gb: gb(current.fallbackMb) })}
    </Button>
  ) : null;

  return (
    <Dialog
      open={pending !== null}
      onClose={dismiss}
      wide
      title={t(current.enough ? "memory.short.titleOk" : "memory.short.title")}
      footer={
        <>
          {!current.enough && !current.auto && (
            <Button variant="ghost" onClick={() => go(current.wantedMb)}>
              {t("memory.short.playAnyway", { gb: gb(current.wantedMb) })}
            </Button>
          )}
          {!current.enough && (
            <Button variant="secondary" disabled={cleaning} onClick={() => void cleanThenRecheck()}>
              <Sparkles className="size-4" aria-hidden />
              {cleaning ? t("memory.clean.busy") : t("memory.clean.button")}
            </Button>
          )}
          {primary}
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-sm">
          {current.enough
            ? t("memory.short.introOk", { gb: gb(current.wantedMb) })
            : current.auto
              ? t(current.fromPack ? "memory.short.introPack" : "memory.short.introAuto", {
                  name,
                  mods: current.mods,
                  wanted: gb(current.wantedMb),
                  spare: gb(current.spareMb),
                })
              : t("memory.short.introManual", {
                  wanted: gb(current.wantedMb),
                  spare: gb(current.spareMb),
                })}
        </p>
        <MemoryBar
          totalMb={current.totalMb}
          availableMb={current.availableMb}
          gameMb={current.wantedMb}
        />
        {!current.enough && (
          <section className="flex flex-col gap-2">
            <h3 className="font-medium text-fg-muted text-xs">
              {t("memory.short.programs", { gb: gb(shortMb) })}
            </h3>
            {programs === null ? (
              <p className="text-fg-muted text-sm">{t("memory.short.programsLoading")}</p>
            ) : shown.length === 0 ? (
              <p className="text-fg-muted text-sm">{t("memory.short.programsNone")}</p>
            ) : (
              <ul className="flex flex-col divide-y divide-line rounded-panel border border-line">
                {shown.map((program) => (
                  <li key={program.name} className="flex items-center gap-3 px-3 py-2">
                    <p className="min-w-0 flex-1 truncate text-sm">{program.name}</p>
                    <span className="shrink-0 text-fg-muted text-xs tabular-nums">
                      {t("memory.short.about", { gb: gb(program.memoryMb) })}
                    </span>
                  </li>
                ))}
              </ul>
            )}
            <p className="text-fg-muted text-xs">{t("memory.short.note")}</p>
          </section>
        )}
      </div>
    </Dialog>
  );
}
