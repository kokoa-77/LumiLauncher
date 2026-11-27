import type { LoaderKind } from "./bindings";

export const LOADER_LABELS: Record<LoaderKind, string> = {
  neoforge: "NeoForge",
  fabric: "Fabric",
  forge: "Forge",
  quilt: "Quilt",
  legacyfabric: "Legacy Fabric",
  babric: "Babric",
};

function parseReleaseVersion(mc: string): [number, number, number] | null {
  if (!/^\d+\.\d+(?:\.\d+)?$/.test(mc)) return null;
  const [maj, min, pat = "0"] = mc.split(".");
  return [Number(maj), Number(min), Number(pat)];
}

function isModernSnapshotForFabric(mc: string): boolean {
  const weekly = /^(\d{2})w(\d{2})/.exec(mc);
  if (weekly) {
    const year = Number(weekly[1]);
    const week = Number(weekly[2]);
    return year > 18 || (year === 18 && week >= 43);
  }
  const dashIdx = mc.search(/[-_]/);
  if (dashIdx > 0) {
    const base = parseReleaseVersion(mc.slice(0, dashIdx));
    if (base) return (base[0] === 1 && base[1] >= 14) || base[0] >= 26;
  }
  return false;
}

/** 与后端 `LoaderKind::installable_for` 保持一致的版本兼容矩阵。 */
export function isLoaderCompatible(kind: LoaderKind, mc: string): boolean {
  if (!mc) return false;
  const rel = parseReleaseVersion(mc);
  switch (kind) {
    case "neoforge":
      return (
        rel !== null &&
        ((rel[0] === 1 && ((rel[1] === 20 && rel[2] >= 1) || rel[1] >= 21)) || rel[0] >= 26)
      );
    case "forge":
      return (
        rel !== null &&
        ((rel[0] === 1 && ((rel[1] === 5 && rel[2] >= 2) || rel[1] >= 6)) || rel[0] >= 26)
      );
    case "fabric":
    case "quilt":
      return (
        (rel !== null && ((rel[0] === 1 && rel[1] >= 14) || rel[0] >= 26)) ||
        isModernSnapshotForFabric(mc)
      );
    case "legacyfabric":
      return (
        rel !== null &&
        rel[0] === 1 &&
        ((rel[1] === 3 && rel[2] >= 2) || (rel[1] >= 4 && rel[1] <= 13))
      );
    case "babric":
      return mc === "b1.7.3";
  }
}

/** 根据所选加载器与可用列表选出当前最合理的加载器选择（不兼容时平滑回退原版）。 */
export function fallbackLoaderChoice(
  current: "vanilla" | LoaderKind,
  mc: string,
): "vanilla" | LoaderKind {
  if (current === "vanilla" || isLoaderCompatible(current, mc)) return current;
  return "vanilla";
}
