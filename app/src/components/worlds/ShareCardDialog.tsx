import { Copy, Download, Sparkles } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { Segmented } from "@/components/ui/Segmented";
import { Switch } from "@/components/ui/Switch";
import { unwrap } from "@/lib/api";
import { commands, type InstanceSummary, type WorldView } from "@/lib/bindings";
import { imageAt } from "@/lib/cover";
import { worldFacts } from "@/lib/format";
import { reportError } from "@/lib/report";

export interface ShareCardTarget {
  instance: InstanceSummary;
  world: WorldView;
}

type CardTheme = "dark" | "light";

const CARD_W = 1200;
const CARD_H = 675;

function readCssVar(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  const val = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return val || fallback;
}

function loadImage(url: string): Promise<HTMLImageElement | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = () => resolve(null);
    img.src = url;
  });
}

function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}

function truncateText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let cut = text;
  while (cut.length > 1 && ctx.measureText(`${cut}…`).width > maxWidth) {
    cut = cut.slice(0, -1);
  }
  return `${cut}…`;
}

function drawPill(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  text: string,
  fill: string,
  stroke: string,
  textColor: string,
  fontSize = 18,
): number {
  ctx.save();
  ctx.font = `500 ${fontSize}px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif`;
  const padX = 16;
  const h = fontSize + 18;
  const w = Math.ceil(ctx.measureText(text).width) + padX * 2;
  roundRectPath(ctx, x, y, w, h, h / 2);
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.strokeStyle = stroke;
  ctx.lineWidth = 1.2;
  ctx.stroke();
  ctx.fillStyle = textColor;
  ctx.textBaseline = "middle";
  ctx.fillText(text, x + padX, y + h / 2 + 0.5);
  ctx.restore();
  return w;
}

/**
 * 世界分享卡片弹窗（D25e）：
 * 纯前端 Canvas 1200×675 高清渲染，结合世界封面、世界名、游玩事实、笔记与核心模组 Top 6，
 * 支持「深色微光 / 浅色微光」切换、一键复制 PNG 到剪贴板或保存到本地文件。
 */
export function ShareCardDialog({
  target,
  onClose,
}: {
  target: ShareCardTarget | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [theme, setTheme] = useState<CardTheme>("dark");
  const [showMods, setShowMods] = useState(true);
  const [mods, setMods] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const instanceId = target?.instance.id ?? null;

  useEffect(() => {
    if (!instanceId) {
      setMods([]);
      return;
    }
    let cancelled = false;
    commands
      .contentList(instanceId, "mod")
      .then(unwrap)
      .then((list) => {
        if (cancelled) return;
        const enabled = list
          .filter((item) => item.enabled)
          .map((item) => item.titleZh ?? item.title);
        setMods(enabled);
      })
      .catch(() => {
        if (!cancelled) setMods([]);
      });
    return () => {
      cancelled = true;
    };
  }, [instanceId]);

  const draw = useCallback(async () => {
    const canvas = canvasRef.current;
    if (!canvas || !target) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const { instance, world } = target;
    const accent = readCssVar("--accent", "oklch(0.75 0.14 55)");
    const isDark = theme === "dark";

    const bgBase = isDark ? "oklch(0.16 0.015 260)" : "oklch(0.96 0.008 75)";
    const bgDeep = isDark ? "oklch(0.11 0.014 260)" : "oklch(0.90 0.014 75)";
    const textMain = isDark ? "oklch(0.98 0.005 80)" : "oklch(0.19 0.018 260)";
    const textMuted = isDark ? "oklch(0.84 0.01 80 / 0.85)" : "oklch(0.32 0.018 260 / 0.82)";
    const pillBg = isDark ? "oklch(0.18 0.012 260 / 0.72)" : "oklch(0.99 0.005 80 / 0.82)";
    const pillBorder = isDark ? "oklch(0.98 0.005 80 / 0.16)" : "oklch(0.2 0.015 260 / 0.14)";
    const frameStroke = isDark ? "oklch(0.98 0.005 80 / 0.18)" : "oklch(0.2 0.015 260 / 0.14)";

    ctx.clearRect(0, 0, CARD_W, CARD_H);

    // 1. 背景底色渐变
    const baseGrad = ctx.createLinearGradient(0, 0, CARD_W, CARD_H);
    baseGrad.addColorStop(0, bgBase);
    baseGrad.addColorStop(1, bgDeep);
    ctx.fillStyle = baseGrad;
    ctx.fillRect(0, 0, CARD_W, CARD_H);

    // 2. 世界封面图（等比裁切铺满）
    const imgUrl = world.imageUrl ? imageAt(world.imageUrl, 1280) : null;
    const coverImg = imgUrl ? await loadImage(imgUrl) : null;
    if (coverImg && coverImg.naturalWidth > 0 && coverImg.naturalHeight > 0) {
      const scale = Math.max(CARD_W / coverImg.naturalWidth, CARD_H / coverImg.naturalHeight);
      const dw = coverImg.naturalWidth * scale;
      const dh = coverImg.naturalHeight * scale;
      const dx = (CARD_W - dw) / 2;
      const dy = (CARD_H - dh) / 2;
      ctx.save();
      ctx.globalAlpha = isDark ? 0.92 : 0.78;
      ctx.drawImage(coverImg, dx, dy, dw, dh);
      ctx.restore();
    }

    // 3. 实例主色微光晕 + 底部非线性遮罩（保证文字高对比度）
    const glow = ctx.createRadialGradient(
      CARD_W * 0.18,
      CARD_H * 0.14,
      20,
      CARD_W * 0.18,
      CARD_H * 0.14,
      520,
    );
    glow.addColorStop(0, accent);
    glow.addColorStop(1, "transparent");
    ctx.save();
    ctx.globalAlpha = isDark ? 0.24 : 0.2;
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, CARD_W, CARD_H);
    ctx.restore();

    const scrim = ctx.createLinearGradient(0, 0, 0, CARD_H);
    if (isDark) {
      scrim.addColorStop(0, "oklch(0.12 0.012 260 / 0.36)");
      scrim.addColorStop(0.42, "oklch(0.12 0.012 260 / 0.28)");
      scrim.addColorStop(0.76, "oklch(0.11 0.014 260 / 0.80)");
      scrim.addColorStop(1, "oklch(0.10 0.014 260 / 0.94)");
    } else {
      scrim.addColorStop(0, "oklch(0.98 0.006 80 / 0.32)");
      scrim.addColorStop(0.4, "oklch(0.98 0.006 80 / 0.22)");
      scrim.addColorStop(0.72, "oklch(0.97 0.008 80 / 0.84)");
      scrim.addColorStop(1, "oklch(0.96 0.008 80 / 0.95)");
    }
    ctx.fillStyle = scrim;
    ctx.fillRect(0, 0, CARD_W, CARD_H);

    // 4. 若干颗克制微光尘埃
    const motes = [
      [140, 120, 4, 0.55],
      [280, 210, 3, 0.35],
      [960, 150, 5, 0.45],
      [1080, 260, 3, 0.35],
      [820, 480, 4, 0.3],
    ] as const;
    for (const [mx, my, r, alpha] of motes) {
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.fillStyle = accent;
      ctx.beginPath();
      ctx.arc(mx, my, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }

    // 5. 顶栏：Lumi 微光徽章 + 游戏版本/加载器胶囊
    const pad = 54;
    ctx.save();
    roundRectPath(ctx, pad, pad, 14, 14, 7);
    ctx.fillStyle = accent;
    ctx.fill();
    ctx.font = '600 20px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif';
    ctx.fillStyle = textMain;
    ctx.textBaseline = "middle";
    ctx.fillText(t("shareCard.brandTag"), pad + 24, pad + 7);
    ctx.restore();

    const loaderLabel = instance.loader
      ? `${instance.gameVersion} · ${instance.loader}`
      : `${instance.gameVersion} · ${t("discover.loader.vanilla")}`;
    ctx.save();
    ctx.font = '500 18px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif';
    const badgeW = Math.ceil(ctx.measureText(loaderLabel).width) + 32;
    drawPill(ctx, CARD_W - pad - badgeW, pad - 10, loaderLabel, pillBg, pillBorder, textMain, 18);
    ctx.restore();

    // 6. 底部主信息：世界名 + 事实 + 世界笔记 + 核心模组 Top 6
    const shownMods = showMods ? mods.slice(0, 6) : [];
    const extraMods = showMods && mods.length > 6 ? mods.length - 6 : 0;
    const hasModsRow = shownMods.length > 0;
    const modsY = CARD_H - pad - 36;
    const noteY = hasModsRow ? modsY - 38 : CARD_H - pad - 10;
    const hasNote = Boolean(world.note?.text);
    const factsY = hasNote ? noteY - 36 : hasModsRow ? modsY - 40 : CARD_H - pad - 14;
    const titleY = factsY - 60;

    // 世界名
    ctx.save();
    ctx.font = '700 58px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif';
    ctx.fillStyle = textMain;
    ctx.textBaseline = "alphabetic";
    const worldTitle = truncateText(ctx, world.name, CARD_W - pad * 2);
    ctx.fillText(worldTitle, pad, titleY);
    ctx.restore();

    // 世界事实（游戏模式 · 天数 · 时长）
    const factsText = [instance.name, ...worldFacts(world, t)].filter(Boolean).join(" · ");
    ctx.save();
    ctx.font = '500 22px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif';
    ctx.fillStyle = textMuted;
    ctx.textBaseline = "alphabetic";
    ctx.fillText(truncateText(ctx, factsText, CARD_W - pad * 2), pad, factsY);
    ctx.restore();

    // 世界笔记（如有）
    if (world.note?.text) {
      ctx.save();
      ctx.font = '500 19px "Segoe UI Variable", "Microsoft YaHei UI", system-ui, sans-serif';
      ctx.fillStyle = accent;
      ctx.textBaseline = "alphabetic";
      const noteText =
        world.note.kind === "todo" ? t("notes.next", { text: world.note.text }) : world.note.text;
      ctx.fillText(truncateText(ctx, noteText, CARD_W - pad * 2), pad, noteY);
      ctx.restore();
    }

    // 核心模组 Top 6 胶囊
    if (hasModsRow) {
      let cursorX = pad;
      const maxX = CARD_W - pad - 140;
      for (const modName of shownMods) {
        const label = modName.length > 18 ? `${modName.slice(0, 17)}…` : modName;
        const estimated = label.length * 14 + 34;
        if (cursorX + estimated > maxX) break;
        const w = drawPill(ctx, cursorX, modsY, label, pillBg, pillBorder, textMain, 16);
        cursorX += w + 10;
      }
      if (extraMods > 0) {
        drawPill(
          ctx,
          cursorX,
          modsY,
          t("shareCard.modsMore", { n: extraMods }),
          pillBg,
          accent,
          accent,
          16,
        );
      }
    }

    // 7. 1px 微光内描边
    ctx.save();
    roundRectPath(ctx, 12, 12, CARD_W - 24, CARD_H - 24, 22);
    ctx.strokeStyle = frameStroke;
    ctx.lineWidth = 1.5;
    ctx.stroke();
    ctx.restore();
  }, [target, theme, showMods, mods, t]);

  useEffect(() => {
    void draw();
  }, [draw]);

  const copyCard = async () => {
    const canvas = canvasRef.current;
    if (!canvas || busy) return;
    setBusy(true);
    try {
      const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
      if (!blob || typeof ClipboardItem === "undefined" || !navigator.clipboard?.write) {
        toast.info(t("shareCard.copyFailed"));
        return;
      }
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast.success(t("shareCard.copied"));
    } catch {
      toast.info(t("shareCard.copyFailed"));
    } finally {
      setBusy(false);
    }
  };

  const saveCard = async () => {
    const canvas = canvasRef.current;
    if (!canvas || !target || busy) return;
    setBusy(true);
    try {
      const dataUrl = canvas.toDataURL("image/png");
      const suggested = `${target.world.name}-lumi-card.png`;
      const saved = unwrap(await commands.worldsSaveShareCard(suggested, dataUrl));
      if (saved) toast.success(t("shareCard.saved"));
    } catch (error) {
      reportError(error, "保存世界分享卡片");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={target !== null}
      onClose={onClose}
      wide
      title={t("shareCard.title")}
      footer={
        <>
          <Button variant="secondary" disabled={busy} onClick={() => void copyCard()}>
            <Copy className="size-4" aria-hidden />
            {t("shareCard.copy")}
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void saveCard()}>
            <Download className="size-4" aria-hidden />
            {t("shareCard.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-fg-muted text-sm">{t("shareCard.subtitle")}</p>
        <div className="overflow-hidden rounded-panel border border-line bg-surface-2 shadow-raised">
          <canvas
            ref={canvasRef}
            width={CARD_W}
            height={CARD_H}
            className="block aspect-video w-full"
          />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <Segmented<CardTheme>
            label={t("shareCard.title")}
            value={theme}
            onChange={setTheme}
            options={[
              { value: "dark", label: t("shareCard.themeDark") },
              { value: "light", label: t("shareCard.themeLight") },
            ]}
          />
          {mods.length > 0 && (
            <label className="flex cursor-pointer items-center gap-2.5 text-sm">
              <Sparkles className="size-4 text-accent" aria-hidden />
              <span>{t("shareCard.showMods")}</span>
              <Switch checked={showMods} onChange={setShowMods} label={t("shareCard.showMods")} />
            </label>
          )}
        </div>
      </div>
    </Dialog>
  );
}
