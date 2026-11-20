import { useEffect, useRef } from "react";
import { headPixels } from "@/lib/avatar";
import { logError } from "@/lib/report";
import { drawHead, loadSkinImage } from "@/lib/skin2d";

/**
 * 玩家的像素头像：有皮肤就画皮肤上的脸（带帽子那层）；没有皮肤时按名字画一个固定配色的小脸
 * （游戏里没皮肤是默认的 Steve、Alex 那些，那是游戏的美术，不能放进启动器）。
 */
export function PlayerHead({
  name,
  skin,
  className = "",
}: {
  name: string;
  /** 皮肤图片（`data:` 地址） */
  skin?: string | null;
  className?: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (!skin) return;
    let cancelled = false;
    loadSkinImage(skin)
      .then((image) => {
        if (!cancelled && canvasRef.current) drawHead(canvasRef.current, image);
      })
      .catch((error: unknown) => logError(error, "画头像"));
    return () => {
      cancelled = true;
    };
  }, [skin]);

  if (skin) {
    return (
      <canvas
        ref={canvasRef}
        width={8}
        height={8}
        className={`shrink-0 rounded-[3px] [image-rendering:pixelated] ${className}`}
        aria-hidden
      />
    );
  }
  return (
    <svg
      viewBox="0 0 8 8"
      className={`shrink-0 rounded-[3px] [shape-rendering:crispEdges] ${className}`}
      aria-hidden
    >
      {headPixels(name).map(([x, y, w, h, fill]) => (
        <rect key={`${x}-${y}-${w}-${h}`} x={x} y={y} width={w} height={h} fill={fill} />
      ))}
    </svg>
  );
}
