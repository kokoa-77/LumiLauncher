import { useEffect, useRef } from "react";
import type { SkinModel } from "@/lib/bindings";
import { logError } from "@/lib/report";
import { drawBody, loadSkinImage, type Side } from "@/lib/skin2d";

/**
 * 2D 全身人物：直接按皮肤文件的像素画（16×32 的小画布，CSS 放大，像素不糊）。
 * 大小由 `className` 定（宽高比 1:2）。
 */
export function SkinFigure({
  skin,
  model,
  side = "front",
  className = "",
}: {
  skin: string;
  model: SkinModel;
  side?: Side;
  className?: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    let cancelled = false;
    loadSkinImage(skin)
      .then((image) => {
        if (!cancelled && canvasRef.current) drawBody(canvasRef.current, image, model, side);
      })
      .catch((error: unknown) => logError(error, "画 2D 人物"));
    return () => {
      cancelled = true;
    };
  }, [skin, model, side]);
  return (
    <canvas
      ref={canvasRef}
      width={16}
      height={32}
      className={`aspect-[1/2] [image-rendering:pixelated] ${className}`}
      aria-hidden
    />
  );
}
