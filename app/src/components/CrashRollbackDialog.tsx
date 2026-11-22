import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type RollbackOffer } from "@/lib/bindings";
import { logError, reportError } from "@/lib/report";
import { playLumiSound } from "@/lib/sound";
import { useGameStore } from "@/stores/game";
import { SpotArt } from "./art/SpotArt";
import { formatTime } from "./HistoryDialog";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { HoverHint } from "./ui/HoverCard";
import { Tag } from "./ui/Tag";

/** 弹窗里最多列几项改动，多的折成「还有 N 项」（不做长列表） */
const MAX_SHOWN = 8;

interface Pending {
  instanceId: string;
  offer: RollbackOffer;
}

/**
 * 「永不玩坏」：游戏崩溃后，如果和「能玩的时候」比改过东西，问一句要不要回去。
 * 回去只动模组 / 光影 / 材质包；拿掉的文件留在内容仓库里，可以在改动历史里撤销。
 */
export function CrashRollbackDialog() {
  const { t } = useTranslation();
  const sessions = useGameStore((s) => s.sessions);
  const instances = useGameStore((s) => s.instances);
  const launch = useGameStore((s) => s.launch);
  const bumpContent = useGameStore((s) => s.bumpContent);
  const [pending, setPending] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  // 每次崩溃只问一次（按实例 + 那次启动的开始时间区分）
  const asked = useRef<Record<string, number | null>>({});

  useEffect(() => {
    for (const session of Object.values(sessions)) {
      if (session.state.kind !== "crashed") continue;
      if (asked.current[session.instanceId] === session.startedAt) continue;
      asked.current[session.instanceId] = session.startedAt;
      const instanceId = session.instanceId;
      void (async () => {
        try {
          const offer = unwrap(await commands.contentCrashOffer(instanceId));
          if (offer) setPending({ instanceId, offer });
        } catch (error) {
          // 查不出来不打扰用户：崩溃提示照常显示，只是没有「回到改动前」
          logError(error, "检查崩溃前的改动");
        }
      })();
    }
  }, [sessions]);

  const close = () => {
    if (!busy) setPending(null);
  };

  const restore = async () => {
    if (!pending) return;
    const { instanceId } = pending;
    setBusy(true);
    try {
      const count = unwrap(await commands.contentRestoreSnapshot(instanceId));
      bumpContent();
      setPending(null);
      playLumiSound("rollback");
      toast.success(t("safety.restored", { n: count }), {
        action: { label: t("safety.play"), onClick: () => void launch(instanceId) },
      });
    } catch (error) {
      reportError(error, "回到改动前");
    } finally {
      setBusy(false);
    }
  };

  const offer = pending?.offer;
  const name = instances.find((i) => i.id === pending?.instanceId)?.name ?? pending?.instanceId;
  const shown = offer?.changes.slice(0, MAX_SHOWN) ?? [];
  const hidden = offer?.changes.slice(MAX_SHOWN) ?? [];

  return (
    <Dialog
      open={!!pending}
      onClose={close}
      wide
      title={t(offer?.verified ? "safety.titleVerified" : "safety.title")}
      footer={
        <>
          <Button variant="ghost" disabled={busy} onClick={close}>
            {t("safety.later")}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void restore()}>
            {t(offer?.verified ? "safety.restoreVerified" : "safety.restore")}
          </Button>
        </>
      }
    >
      {offer && (
        <div className="flex flex-col gap-3">
          <SpotArt kind="rollback" className="size-24 self-center" />
          <p className="text-fg-muted text-sm">
            {t(offer.verified ? "safety.introVerified" : "safety.intro", {
              name,
              time: formatTime(offer.at ?? 0),
            })}
          </p>
          <ul className="flex flex-col divide-y divide-line rounded-panel border border-line">
            {shown.map((change) => (
              <li
                key={`${change.kind}-${change.change}-${change.title}`}
                className="flex items-center gap-3 px-3 py-2"
              >
                <Tag>{t(`safety.change.${change.change}`)}</Tag>
                <p className="min-w-0 flex-1 truncate text-sm">
                  {change.title}
                  {change.kind !== "mod" && (
                    <span className="text-fg-muted"> · {t(`kind.${change.kind}`)}</span>
                  )}
                </p>
                <span className="shrink-0 text-fg-muted text-xs">
                  {change.fromVersion && change.toVersion
                    ? `${change.fromVersion} → ${change.toVersion}`
                    : (change.toVersion ?? change.fromVersion ?? "")}
                </span>
              </li>
            ))}
            {hidden.length > 0 && (
              <li className="px-3 py-2 text-fg-muted text-xs">
                <HoverHint hint={hidden.map((change) => change.title).join("、")}>
                  {t("safety.more", { n: hidden.length })}
                </HoverHint>
              </li>
            )}
          </ul>
          <p className="text-fg-muted text-xs">{t("safety.note")}</p>
        </div>
      )}
    </Dialog>
  );
}
