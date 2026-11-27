import {
  Archive,
  Blocks,
  Box,
  CircleCheck,
  CircleHelp,
  FileCode,
  FolderOpen,
  Globe,
  Image as ImageIcon,
  Package,
  Shirt,
  Sparkles,
} from "lucide-react";
import { type ComponentType, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import {
  commands,
  type DropCheck,
  type DropItem,
  type DropKind,
  type ModFit,
  type WorldView,
} from "@/lib/bindings";
import { formatSize } from "@/lib/format";
import { logError, reportError } from "@/lib/report";
import { useDropStore } from "@/stores/drops";
import { useFolderStore } from "@/stores/folders";
import { useGameStore } from "@/stores/game";
import { useDropTarget } from "./DropOverlay";
import { TargetPicker } from "./discover/TargetPicker";
import { ImportPackDialog } from "./ImportPackDialog";
import { Button } from "./ui/Button";
import { Checkbox } from "./ui/Checkbox";
import { Dialog } from "./ui/Dialog";
import { Select } from "./ui/Select";
import { VirtualList } from "./VirtualList";

const KIND_ICON: Record<
  DropKind,
  ComponentType<{ className?: string; "aria-hidden"?: boolean }>
> = {
  modpack: Package,
  mod: Box,
  resourcePack: ImageIcon,
  shader: Sparkles,
  dataPack: FileCode,
  world: Globe,
  schematic: Blocks,
  bundle: Archive,
  skin: Shirt,
  unknown: CircleHelp,
};

/** 能装进实例的（整合包另走导入，认不出的不装；一包东西到这里已经拆开了） */
const installable = (item: DropItem) =>
  item.kind !== "modpack" && item.kind !== "unknown" && item.kind !== "bundle";

/** 列表里按种类排（一包东西拆出来几百个时好找） */
const KIND_ORDER: DropKind[] = [
  "modpack",
  "mod",
  "resourcePack",
  "shader",
  "dataPack",
  "world",
  "schematic",
  "bundle",
  "unknown",
];
const byKind = (a: DropItem, b: DropItem) =>
  KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind);

/** 加载器的名字（给人看） */
const LOADER_NAME: Record<string, string> = {
  neoforge: "NeoForge",
  forge: "Forge",
  fabric: "Fabric",
  quilt: "Quilt",
  legacyfabric: "Legacy Fabric",
  babric: "Babric",
};
const loaderName = (id: string) => LOADER_NAME[id] ?? id;

/** 列表里的一行：一个文件，或者「装不了的 N 个」这种小标题 */
type ListRow = { type: "item"; item: DropItem } | { type: "label"; text: string };

/** 这一项装上去有什么要提醒的；`null` 是没问题 */
function useProblem(item: DropItem, check: DropCheck | undefined, loader: string | null) {
  const { t } = useTranslation();
  const fit: ModFit | undefined = check?.fit;
  if (fit?.kind === "noLoader") return t("drop.fit.noLoader");
  if (fit?.kind === "wrongLoader") {
    return t("drop.fit.wrongLoader", {
      needs: fit.needs.map(loaderName).join(" / "),
      has: loader ? loaderName(loader) : t("drop.vanilla"),
    });
  }
  if (fit?.kind === "wrongGame") {
    return fit.supports.length > 0
      ? t("drop.fit.wrongGame", { supports: fit.supports.join("、") })
      : t("drop.fit.wrongGameUnknown");
  }
  const folder = item.folder;
  if (folder?.kind === "gameRoot") {
    return t("drop.folder.gameRoot", {
      launcher: t(`launchers.${folder.root.launcher}`),
      count: folder.root.versions,
    });
  }
  if (folder?.kind === "foreign") {
    const one = folder.instances.length === 1 ? folder.instances[0] : undefined;
    return one
      ? t("drop.folder.foreignOne", { launcher: t(`launchers.${one.launcher}`), name: one.name })
      : t("drop.folder.foreign", { count: folder.instances.length });
  }
  if (item.kind === "unknown" && item.problem) return t(`drop.problem.${item.problem}`);
  return null;
}

/** 别家的游戏目录已经挂上了（移除过的版本也没有）：不用再点 */
const alreadyLinked = (item: DropItem) =>
  item.folder?.kind === "gameRoot" && item.folder.linked && item.folder.root.hidden === 0;

function Row({
  item,
  check,
  checked,
  onToggle,
  loader,
  gameVersion,
  onImport,
  onFolder,
  showFrom,
}: {
  item: DropItem;
  check: DropCheck | undefined;
  checked: boolean;
  onToggle: (checked: boolean) => void;
  loader: string | null;
  gameVersion: string;
  onImport: () => void;
  /** 别家的游戏目录、实例：挂上、导入 */
  onFolder: () => void;
  /** 每行写「在哪一包里」（全都来自同一包时只在上面写一次） */
  showFrom: boolean;
}) {
  const { t } = useTranslation();
  const Icon = item.folder ? FolderOpen : KIND_ICON[item.kind];
  const muted = item.kind === "unknown" && !item.folder;
  const problem = useProblem(item, check, loader);
  const same = check?.duplicate?.kind === "same";
  const other = check?.duplicate?.kind === "otherVersion" ? check.duplicate : null;
  const title = item.title ?? item.name;
  const facts = [
    item.kind === "unknown" ? null : t(`drop.kind.${item.kind}`),
    item.version,
    title !== item.name ? item.name : null,
    item.size ? formatSize(item.size) : null,
    item.from && showFrom ? t("drop.from", { name: item.from }) : null,
  ].filter(Boolean);
  return (
    <div className="flex items-center gap-3 py-2.5">
      {item.icon ? (
        <img src={item.icon} alt="" className="size-9 shrink-0 rounded-row" />
      ) : (
        <div className="grid size-9 shrink-0 place-items-center rounded-row bg-surface-hover text-fg-muted">
          <Icon className="size-4" aria-hidden />
        </div>
      )}
      <div className="min-w-0 flex-1">
        <p className={`truncate font-medium text-sm ${muted ? "text-fg-muted" : ""}`}>{title}</p>
        <p className="truncate text-fg-muted text-xs">{facts.join(" · ")}</p>
        {problem && (
          <p className={`text-xs ${item.kind === "unknown" ? "text-fg-muted" : "text-warning"}`}>
            {problem}
            {check?.fit.kind === "wrongGame" &&
              ` ${t("drop.fit.instanceIs", { version: gameVersion })}`}
          </p>
        )}
        {other && (
          <p className="truncate text-fg-muted text-xs">
            {t("drop.replaces", { name: other.version ?? other.file })}
          </p>
        )}
      </div>
      {alreadyLinked(item) ? (
        <span className="flex shrink-0 items-center gap-1 rounded-tag bg-accent-soft px-2 py-0.5 text-accent text-xs">
          <CircleCheck className="size-3.5" aria-hidden />
          {t("drop.folder.linked")}
        </span>
      ) : item.folder ? (
        <Button variant="secondary" size="sm" onClick={onFolder}>
          {item.folder.kind === "gameRoot" ? t("drop.folder.add") : t("drop.folder.import")}
        </Button>
      ) : item.kind === "modpack" ? (
        <Button variant="secondary" size="sm" onClick={onImport}>
          <Archive className="size-4" aria-hidden />
          {t("drop.importPack")}
        </Button>
      ) : same ? (
        <span className="flex shrink-0 items-center gap-1 rounded-tag bg-accent-soft px-2 py-0.5 text-accent text-xs">
          <CircleCheck className="size-3.5" aria-hidden />
          {t("drop.same")}
        </span>
      ) : installable(item) ? (
        <Checkbox checked={checked} onChange={onToggle} label={t("drop.pick", { name: title })} />
      ) : null}
    </div>
  );
}

/**
 * 「装到哪」：拖进来的文件一行一个。上面选实例（默认是现在看着的实例），
 * 有数据包时再选装进哪个世界。模组先查和实例对不对得上：对不上的标黄、默认不勾；
 * 同一个模组的旧版本会被换掉；一模一样的标「已经装过了」。认不出的写明为什么不装。
 */
export function DropDialog() {
  const { t } = useTranslation();
  const items = useDropStore((s) => s.items);
  const rests = useDropStore((s) => s.rests);
  const pack = useDropStore((s) => s.pack);
  const closeItems = useDropStore((s) => s.closeItems);
  const openPack = useDropStore((s) => s.openPack);
  const closePack = useDropStore((s) => s.closePack);
  const takeItem = useDropStore((s) => s.takeItem);
  const instances = useGameStore((s) => s.instances);
  const recent = useGameStore((s) => s.recentWorlds);
  const defaultTarget = useDropTarget();
  const [target, setTarget] = useState<string>("");
  const [checks, setChecks] = useState<Record<string, DropCheck>>({});
  const [checking, setChecking] = useState(false);
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [worlds, setWorlds] = useState<WorldView[]>([]);
  const [world, setWorld] = useState("");
  const [busy, setBusy] = useState(false);

  const worldOf = useMemo(
    () => new Map((recent ?? []).map((r) => [r.instanceId, r.world])),
    [recent],
  );
  const open = items !== null && items.length > 0;
  const list = items ?? [];
  const toInstall = list.filter(installable);
  const needsWorld = toInstall.some((i) => i.kind === "dataPack");
  const instance = instances.find((i) => i.id === target);
  const loader = instance?.loader ? "neoforge" : null;

  // 打开时用默认的实例
  useEffect(() => {
    if (open) setTarget((current) => current || defaultTarget || "");
    if (!open) {
      setTarget("");
      setChecks({});
      setPicked({});
    }
  }, [open, defaultTarget]);

  // 换实例就重新检查：对不对得上、装没装过
  const tokens = toInstall.map((i) => i.token).join(",");
  useEffect(() => {
    if (!open || !target || !tokens) return;
    let cancelled = false;
    setChecking(true);
    commands
      .dropsCheck(tokens.split(","), target)
      .then(unwrap)
      .then((result) => {
        if (cancelled) return;
        const byToken = Object.fromEntries(result.map((c) => [c.token, c]));
        setChecks(byToken);
        // 默认勾上对得上、又没装过的
        setPicked(
          Object.fromEntries(
            result.map((c) => [c.token, c.fit.kind === "fits" && c.duplicate?.kind !== "same"]),
          ),
        );
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        reportError(error, "检查拖进来的文件");
        setPicked(Object.fromEntries(tokens.split(",").map((tk) => [tk, true])));
      })
      .finally(() => !cancelled && setChecking(false));
    return () => {
      cancelled = true;
    };
  }, [open, target, tokens]);

  // 有数据包时读这个实例的世界，默认最近玩的那个
  useEffect(() => {
    if (!open || !needsWorld || !target) return;
    let cancelled = false;
    commands
      .worldsList(target)
      .then(unwrap)
      .then((result) => {
        if (cancelled) return;
        setWorlds(result.worlds);
        setWorld(result.worlds[0]?.folder ?? "");
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        logError(error, "读取实例的存档");
        setWorlds([]);
        setWorld("");
      });
    return () => {
      cancelled = true;
    };
  }, [open, needsWorld, target]);

  const chosen = toInstall.filter((i) => picked[i.token]);
  const blockedByWorld = chosen.some((i) => i.kind === "dataPack") && !world;

  const install = async () => {
    if (!target || chosen.length === 0) return;
    setBusy(true);
    try {
      const jobId = unwrap(
        await commands.dropsInstall(
          target,
          chosen.map((i) => {
            const duplicate = checks[i.token]?.duplicate;
            return {
              token: i.token,
              replace: duplicate?.kind === "otherVersion" ? duplicate.file : null,
            };
          }),
          needsWorld ? world || null : null,
        ),
      );
      closeItems(chosen.map((i) => i.token));
      useGameStore.getState().showTasksFor(jobId);
    } catch (error) {
      reportError(error, "装拖进来的文件");
    } finally {
      setBusy(false);
    }
  };

  // 别家的游戏目录挂上了再从列表里拿走；实例马上拿走，去导入窗口
  const onFolder = (item: DropItem) => {
    const folder = item.folder;
    if (!folder) return;
    if (folder.kind === "foreign") {
      takeItem(item.token);
      useFolderStore.getState().show(folder);
    } else {
      useFolderStore.getState().show(folder, () => takeItem(item.token));
    }
  };

  // 别家的游戏目录、实例虽然装不进实例，但能挂上、导入，不算「装不了」
  const usable = (i: DropItem) => i.kind !== "unknown" || Boolean(i.folder);
  const onlyUnknown =
    toInstall.length === 0 && !list.some((i) => i.kind === "modpack" || Boolean(i.folder));
  // 能装的在上面（按种类排），装不了的单独一组放最后；一包东西拆出来可能几百个，用虚拟列表
  const known = list.filter(usable).sort(byKind);
  const unknown = list.filter((i) => !usable(i));
  const rows: ListRow[] = [
    ...known.map((item) => ({ type: "item" as const, item })),
    ...(unknown.length > 0 && known.length > 0
      ? [{ type: "label" as const, text: t("drop.cannot", { n: unknown.length }) }]
      : []),
    ...unknown.map((item) => ({ type: "item" as const, item })),
  ];
  // 全选：已经装过的不算
  const selectable = toInstall.filter((i) => checks[i.token]?.duplicate?.kind !== "same");
  // 全都是从同一包东西里拆出来的：上面写一次「从哪一包拆出来」，每行就不写了
  const sources = new Set(list.map((i) => i.from));
  const oneBundle = sources.size === 1 ? (list[0]?.from ?? null) : null;
  const allPicked = selectable.length > 0 && selectable.every((i) => picked[i.token]);
  const pickAll = (value: boolean) =>
    setPicked((p) => ({ ...p, ...Object.fromEntries(selectable.map((i) => [i.token, value])) }));

  return (
    <>
      <Dialog
        open={open}
        onClose={() => closeItems()}
        wide
        title={onlyUnknown ? t("drop.titleUnknown") : t("drop.title")}
        footer={
          onlyUnknown ? (
            <Button variant="primary" onClick={() => closeItems()}>
              {t("drop.ok")}
            </Button>
          ) : (
            <>
              <Button variant="ghost" onClick={() => closeItems()}>
                {t("common.cancel")}
              </Button>
              {toInstall.length > 0 && (
                <Button
                  variant="primary"
                  disabled={busy || checking || chosen.length === 0 || blockedByWorld || !instance}
                  onClick={() => void install()}
                >
                  {t("drop.confirm", { n: chosen.length, name: instance?.name ?? "" })}
                </Button>
              )}
            </>
          )
        }
      >
        {toInstall.length > 0 && (
          <div className="mb-3 flex flex-col gap-3">
            <div>
              <TargetPicker
                instances={instances}
                worldOf={worldOf}
                value={target}
                onChange={setTarget}
              />
            </div>
            {needsWorld && (
              <div className="flex items-center gap-3">
                <span className="shrink-0 text-fg-muted text-sm">{t("drop.world")}</span>
                {worlds.length > 0 ? (
                  <div className="w-64">
                    <Select
                      label={t("drop.world")}
                      value={world}
                      onChange={setWorld}
                      options={worlds.map((w) => ({ value: w.folder, label: w.name }))}
                    />
                  </div>
                ) : (
                  <span className="text-sm text-warning">{t("drop.noWorld")}</span>
                )}
              </div>
            )}
          </div>
        )}
        {(selectable.length > 1 || oneBundle) && (
          <div className="flex min-h-7 items-center justify-between gap-3 border-line border-b pb-2 text-fg-muted text-xs">
            <span className="min-w-0 truncate">
              {[
                oneBundle ? t("drop.fromAll", { name: oneBundle, n: list.length }) : null,
                selectable.length > 1
                  ? t("drop.picked", { n: chosen.length, total: selectable.length })
                  : null,
              ]
                .filter(Boolean)
                .join(" · ")}
            </span>
            {selectable.length > 1 && (
              <span className="flex shrink-0 items-center gap-2">
                {t("drop.all")}
                <Checkbox
                  checked={allPicked}
                  mixed={!allPicked && chosen.length > 0}
                  onChange={() => pickAll(!allPicked)}
                  label={t("drop.all")}
                />
              </span>
            )}
          </div>
        )}
        <VirtualList
          items={rows}
          estimate={60}
          className="max-h-96"
          getKey={(row, index) => (row.type === "item" ? row.item.token : `label-${index}`)}
          render={(row, index) =>
            row.type === "label" ? (
              <p className="pt-3 pb-1 text-fg-muted text-xs">{row.text}</p>
            ) : (
              <div
                className={
                  index > 0 && rows[index - 1]?.type === "item" ? "border-line border-t" : ""
                }
              >
                <Row
                  item={row.item}
                  check={checks[row.item.token]}
                  checked={!!picked[row.item.token]}
                  onToggle={(value) => setPicked((p) => ({ ...p, [row.item.token]: value }))}
                  loader={loader}
                  gameVersion={instance?.gameVersion ?? ""}
                  showFrom={!oneBundle}
                  onImport={() => openPack(row.item.token)}
                  onFolder={() => onFolder(row.item)}
                />
              </div>
            )
          }
        />
        {rests.map((rest) => (
          <p key={rest.name} className="mt-2 text-fg-muted text-xs">
            {t("drop.rest", { name: rest.name, n: rest.count })}
          </p>
        ))}
        {checking && <p className="mt-2 text-fg-muted text-xs">{t("drop.checking")}</p>}
      </Dialog>
      <ImportPackDialog
        source={pack ? { kind: "dropped", token: pack } : null}
        onClose={closePack}
      />
    </>
  );
}
