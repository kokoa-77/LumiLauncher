import { Server } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ServerStatus } from "@/lib/bindings";
import { formatCount } from "@/lib/format";
import { HoverHint } from "./ui/HoverCard";

/** 同一个地址正在问的那一次（几个地方同时显示同一个服务器时只问一次；后端还会记 30 秒） */
const asking = new Map<string, Promise<ServerStatus>>();

function ask(address: string, fresh: boolean): Promise<ServerStatus> {
  const key = `${address.trim().toLowerCase()}|${fresh}`;
  const running = asking.get(key);
  if (running) return running;
  const promise = commands.serversStatus(address, fresh).finally(() => asking.delete(key));
  asking.set(key, promise);
  return promise;
}

/**
 * 服务器现在的样子：`undefined` 是还在问。`refresh` 变了就重新问（不用后端记着的上一次）。
 */
export function useServerStatus(address: string | null | undefined, refresh = 0) {
  const [status, setStatus] = useState<ServerStatus | undefined>(undefined);
  useEffect(() => {
    if (!address) return;
    let cancelled = false;
    setStatus(undefined);
    ask(address, refresh > 0)
      // 正常不会是空的；万一是（比如预览页没模拟这个命令）按「回答看不懂」显示，不让页面崩
      .then((next) => !cancelled && setStatus(next ?? { kind: "offline", reason: "badReply" }))
      .catch(() => !cancelled && setStatus({ kind: "offline", reason: "badReply" }));
    return () => {
      cancelled = true;
    };
  }, [address, refresh]);
  return status;
}

/** 延迟分三档：快（主色）、一般（黄）、慢（红） */
function latencyTone(ms: number) {
  if (ms < 150) return "bg-accent";
  if (ms < 400) return "bg-warning";
  return "bg-danger";
}

/**
 * 一行小字：「● 12 / 100 · 45 ms」或「○ 连不上」。鼠标停上去看版本、简介或者连不上的原因。
 */
export function ServerStatusLine({
  status,
  address,
  className = "",
  tone = "default",
}: {
  status: ServerStatus | undefined;
  address: string;
  className?: string;
  tone?: "default" | "hero";
}) {
  const { t } = useTranslation();
  const textTone = tone === "hero" ? "text-on-hero text-sm" : "text-fg-muted text-xs";
  const ringTone = tone === "hero" ? "border-on-hero-muted" : "border-fg-muted";
  const pulseTone = tone === "hero" ? "bg-on-hero-muted" : "bg-line";
  if (status === undefined) {
    return (
      <span className={`inline-flex items-center gap-1.5 ${textTone} ${className}`}>
        <span
          className={`size-2 shrink-0 animate-pulse rounded-full ${pulseTone} motion-reduce:animate-none`}
        />
        {t("server.checking")}
      </span>
    );
  }
  if (status.kind === "offline") {
    return (
      <HoverHint
        hint={`${address}\n${t(`server.reason.${status.reason}`)}`}
        className={`inline-flex items-center gap-1.5 ${textTone} ${className}`}
      >
        <span className={`size-2 shrink-0 rounded-full border ${ringTone}`} />
        {t("server.offline")}
      </HoverHint>
    );
  }
  const line = t("server.online", {
    online: formatCount(status.playersOnline),
    max: formatCount(status.playersMax),
    ms: status.latencyMs,
  });
  // 卡片窄的时候这一行会被截断：悬停时整行也在里面
  const hint = [line, address, status.version, status.motd].filter(Boolean).join("\n");
  return (
    <HoverHint
      hint={hint}
      className={`inline-flex min-w-0 items-center gap-1.5 whitespace-nowrap ${textTone} tabular-nums ${className}`}
    >
      <span className={`size-2 shrink-0 rounded-full ${latencyTone(status.latencyMs)}`} />
      <span className="min-w-0 truncate">{line}</span>
    </HoverHint>
  );
}

/** 服务器的图标：服务器自己给了就用它（按像素显示），没有就是一个服务器小图标 */
export function ServerIcon({
  status,
  className = "size-9",
}: {
  status: ServerStatus | undefined;
  className?: string;
}) {
  const favicon = status?.kind === "online" ? status.favicon : null;
  if (favicon) {
    return (
      <img
        src={favicon}
        alt=""
        className={`${className} shrink-0 rounded-row [image-rendering:pixelated]`}
      />
    );
  }
  return (
    <div
      className={`${className} grid shrink-0 place-items-center rounded-row bg-accent-soft text-accent`}
    >
      <Server className="size-4" aria-hidden />
    </div>
  );
}
