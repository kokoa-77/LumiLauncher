import { Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { isActive, useGameStore } from "@/stores/game";
import { LumiIcon } from "./art/LumiIcon";
import { Button } from "./ui/Button";
import { CtaButton } from "./ui/CtaButton";
import { IconButton } from "./ui/IconButton";

/**
 * 开始游戏 / 运行中可结束。`large` 用在首页和实例页：胶囊形主按钮（一页只有一个）。
 * 给了 `world`（存档文件夹名）就直接进这个存档（Quick Play），`label` 换掉按钮上的字，`sub` 是第二行小字。
 */
export function PlayButton({
  instanceId,
  large = false,
  compact = false,
  world,
  server,
  label,
  sub,
  size = "lg",
}: {
  instanceId: string;
  large?: boolean;
  /** `large` 时主按钮多大：首页用大号，实例页横幅用中号 */
  size?: "md" | "lg";
  /**
   * 只有一个圆形图标（实例库的卡片里，免得把名字挤窄）。
   * 每张卡片都有一个，按「反复出现的操作」画成描边；外面要套 `group`，移到卡片上才铺满颜色。
   */
  compact?: boolean;
  world?: string;
  server?: string;
  label?: string;
  sub?: string;
}) {
  const { t } = useTranslation();
  const session = useGameStore((s) => s.sessions[instanceId]);
  const pending = useGameStore((s) => s.pendingLaunches[instanceId] === true);
  const startLabel = pending ? t("play.starting") : (label ?? t("play.start"));
  const launch = useGameStore((s) => s.launch);
  const requestStop = useGameStore((s) => s.requestStop);
  const active = isActive(session);
  const starting =
    active && (session?.state.kind === "starting" || session?.state.kind === "loading");
  const stopIcon =
    starting || pending ? (
      <LightSpinner className="size-4" />
    ) : (
      <Square className="size-3.5 fill-current" aria-hidden />
    );
  const onStop = (event: { stopPropagation: () => void }) => {
    event.stopPropagation();
    requestStop(instanceId);
  };
  const onStart = (event: { stopPropagation: () => void }) => {
    event.stopPropagation();
    void launch(instanceId, world, server);
  };

  if (large) {
    return active ? (
      <CtaButton
        variant="neutral"
        size={size}
        icon={stopIcon}
        label={starting ? t("play.starting") : t("play.stop")}
        onClick={onStop}
      />
    ) : (
      <CtaButton
        size={size}
        label={startLabel}
        disabled={pending}
        aria-busy={pending}
        icon={pending ? stopIcon : undefined}
        sub={sub}
        burst={Boolean(world || server)}
        onClick={onStart}
      />
    );
  }

  if (compact) {
    return active ? (
      <IconButton
        label={starting ? t("play.starting") : t("play.stop")}
        variant="secondary"
        onClick={onStop}
      >
        {stopIcon}
      </IconButton>
    ) : (
      <IconButton
        label={startLabel}
        disabled={pending}
        aria-busy={pending}
        variant="card"
        onClick={onStart}
      >
        {pending ? (
          <LightSpinner className="size-4" />
        ) : (
          <LumiIcon name="play" className="size-4" />
        )}
      </IconButton>
    );
  }

  if (active) {
    return (
      <Button variant="secondary" onClick={onStop}>
        {stopIcon}
        {starting ? t("play.starting") : t("play.stop")}
      </Button>
    );
  }
  // 列表里每一行都有：描边（「反复出现的操作」）
  return (
    <Button variant="outline" disabled={pending} aria-busy={pending} onClick={onStart}>
      {pending ? <LightSpinner className="size-4" /> : <LumiIcon name="play" className="size-4" />}
      {startLabel}
    </Button>
  );
}
