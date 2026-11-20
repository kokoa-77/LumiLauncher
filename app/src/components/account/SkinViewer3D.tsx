import { useEffect, useRef } from "react";
import type { SkinModel } from "@/lib/bindings";
import { logError } from "@/lib/report";

/**
 * 能转动的 3D 人物（skinview3d，MIT）。只在换皮肤弹窗、首次运行引导里用，用到时才加载（不占首屏）；
 * 流畅模式下外面换成 2D 正反面。拖动转身，滚轮缩放；「减少动效」时不走路、不自动转。
 */
export default function SkinViewer3D({
  skin,
  model,
  width,
  height,
  still = false,
}: {
  skin: string;
  model: SkinModel;
  width: number;
  height: number;
  /** 不放动画（减少动效） */
  still?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<import("skinview3d").SkinViewer | null>(null);

  // 建一次，卸载时释放显卡资源；后面换皮肤、改大小交给下面两个 effect
  useEffect(() => {
    let disposed = false;
    import("skinview3d")
      .then((skinview3d) => {
        if (disposed || !canvasRef.current) return;
        const viewer = new skinview3d.SkinViewer({
          canvas: canvasRef.current,
          width,
          height,
          zoom: 0.85,
          fov: 45,
        });
        viewer.controls.enablePan = false;
        viewerRef.current = viewer;
        void viewer.loadSkin(skin, { model: model === "slim" ? "slim" : "default" });
        if (!still) {
          const walk = new skinview3d.WalkingAnimation();
          walk.speed = 0.6;
          viewer.animation = walk;
          viewer.controls.autoRotate = true;
          viewer.controls.autoRotateSpeed = 1.2;
        }
        viewer.playerObject.rotation.y = -0.45;
      })
      .catch((error: unknown) => logError(error, "加载 3D 皮肤预览"));
    return () => {
      disposed = true;
      viewerRef.current?.dispose();
      viewerRef.current = null;
    };
  }, []);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;
    viewer
      .loadSkin(skin, { model: model === "slim" ? "slim" : "default" })
      ?.catch((error: unknown) => logError(error, "3D 预览换皮肤"));
  }, [skin, model]);

  useEffect(() => {
    viewerRef.current?.setSize(width, height);
  }, [width, height]);

  return <canvas ref={canvasRef} className="cursor-grab active:cursor-grabbing" />;
}
