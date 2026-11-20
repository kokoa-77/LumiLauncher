import { openUrl } from "@tauri-apps/plugin-opener";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { PackSourceRef } from "@/components/ImportPackDialog";
import { InstallDialog } from "@/components/InstallDialog";
import { unwrap } from "@/lib/api";
import {
  type ContentItem,
  type ContentKind,
  commands,
  type JobSnapshot,
  type PlanItem,
} from "@/lib/bindings";
import { type Entry, findInstalled, type InstallKind, sourceKey } from "@/lib/discover";
import { logError, reportError } from "@/lib/report";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";

export type { InstallKind } from "@/lib/discover";

const ImportPackDialog = lazy(async () => ({
  default: (await import("@/components/ImportPackDialog")).ImportPackDialog,
}));

/** 安装按钮的状态：能装 → 正在解析 → 正在装（带进度）→ 装好了；服务器已经加进来时是「进入」 */
export type InstallState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "running"; progress: number | null }
  | { kind: "done" }
  | { kind: "joined" }
  /** 作者不许第三方启动器下载的地图：按钮变成「去下载」，点了打开平台页面 */
  | { kind: "manual" };

/** 这个项目最近的一个安装任务（任务键和后端一致） */
function latestJob(
  jobs: Record<string, JobSnapshot>,
  entry: Entry,
  kind: InstallKind,
  instanceId: string,
) {
  const keys = new Set(
    Object.entries(entry.ids).map(([source, id]) => {
      if (kind === "modpack") {
        return `modpack-import:${sourceKey(source as Entry["hit"]["source"])}:${id}`;
      }
      // 要整合包的服务器：装的是它的整合包（大多是服务器自己，也可能是别人发的）
      if (kind === "server") {
        return `modpack-import:modrinth:${entry.hit.server?.modpackProject ?? id}`;
      }
      if (kind === "map") return `map-install:${instanceId}:${id}`;
      return `content:${instanceId}:${id}`;
    }),
  );
  return Object.values(jobs)
    .filter((j) => keys.has(j.key))
    .sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0))[0];
}

/**
 * 实例里已经装的内容（「已安装」标记用）。这个实例有内容安装任务装完时重新读一次。
 * `kinds` 为空时不读。
 */
export function useInstalledContent(instanceId: string, kinds: ContentKind[]) {
  const jobs = useGameStore((s) => s.jobs);
  const [items, setItems] = useState<Partial<Record<ContentKind, ContentItem[]>>>({});
  const doneCount = useMemo(
    () =>
      Object.values(jobs).filter(
        (j) => j.state === "succeeded" && j.key.startsWith(`content:${instanceId}:`),
      ).length,
    [jobs, instanceId],
  );
  const kindsKey = kinds.join(",");
  // doneCount 是「装完了一个」的信号，变了就重读
  useEffect(() => {
    if (!instanceId || kindsKey === "") {
      setItems({});
      return;
    }
    let cancelled = false;
    const wanted = kindsKey.split(",") as ContentKind[];
    void Promise.all(
      wanted.map(
        async (kind) => [kind, unwrap(await commands.contentList(instanceId, kind))] as const,
      ),
    )
      .then((pairs) => {
        if (!cancelled) setItems(Object.fromEntries(pairs));
      })
      .catch((error: unknown) => reportError(error, "读取实例里已装的内容"));
    return () => {
      cancelled = true;
    };
  }, [instanceId, kindsKey, doneCount]);
  return items;
}

const sameAddress = (address: string) => address.trim().toLowerCase();

/**
 * 资源中心的安装流程（列表和详情页共用）：
 * - 整合包：弹出导入窗口，装成新实例
 * - 模组、资源包、光影包：先解析（带上前置）；只有它自己、能直接下载时一点就装，
 *   要顺带装前置、光影加载器或者要手动下载时，先列出来让你确认
 * - 地图：下载解压进实例的存档（后台任务）
 * - 服务器：已经在这个实例的多人游戏列表里就直接进；要整合包的先装整合包（新实例，装好自动加服务器）；
 *   其余的加进这个实例的多人游戏列表
 */
export function useInstall(instanceId: string, instanceName: string) {
  const { t } = useTranslation();
  const jobs = useGameStore((s) => s.jobs);
  const launch = useGameStore((s) => s.launch);
  // 设置决定确认弹窗中前置的初始勾选。
  const autoDependencies = useAppStore((s) => s.settings?.download.autoDependencies ?? true);
  // 连着点了好几个（D17l 验收时发现）：要确认的一个一个弹，后来的不顶掉前面的
  const [plans, setPlans] = useState<
    { items: PlanItem[]; selectDependencies: boolean; instanceId: string; instanceName: string }[]
  >([]);
  const plan = plans[0] ?? null;
  const hasPlan = (hit: Entry["hit"]) =>
    plans.some(
      (plan) =>
        plan.instanceId === instanceId &&
        plan.items.some(
          (item) =>
            item.reason === "requested" &&
            item.projectId === hit.projectId &&
            (item.source ?? "modrinth") === hit.source,
        ),
    );
  /** 正在解析的项目（可以好几个一起） */
  const [resolving, setResolving] = useState<ReadonlySet<string>>(() => new Set());
  const [installing, setInstalling] = useState(false);
  const [packSource, setPackSource] = useState<PackSourceRef | null>();
  /** 这个实例多人游戏列表里已有的服务器地址（小写） */
  const [joined, setJoined] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    if (!instanceId) {
      setJoined(new Set());
      return;
    }
    let cancelled = false;
    commands
      .serversList(instanceId)
      .then(unwrap)
      .then((list) => {
        if (!cancelled) setJoined(new Set(list.map((s) => sameAddress(s.address))));
      })
      .catch((error: unknown) => logError(error, "读取实例的服务器列表"));
    return () => {
      cancelled = true;
    };
  }, [instanceId]);

  const markResolving = (key: string, on: boolean) =>
    setResolving((prev) => {
      const next = new Set(prev);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  /** 确认窗里的这一个处理完了（装了或者不装），弹下一个 */
  const nextPlan = () => setPlans((prev) => prev.slice(1));

  /** 开始装；成功返回 true */
  const install = async (targetId: string, items: PlanItem[]) => {
    setInstalling(true);
    try {
      unwrap(await commands.contentInstall(targetId, items));
      toast.success(t("install.started", { name: items[0]?.title ?? "" }));
      return true;
    } catch (error) {
      reportError(error, "安装资源");
      return false;
    } finally {
      setInstalling(false);
    }
  };

  const joinServer = async (entry: Entry) => {
    const { hit } = entry;
    const server = hit.server;
    if (!server) return;
    if (instanceId && joined.has(sameAddress(server.address))) {
      await launch(instanceId, undefined, server.address);
      return;
    }
    if (server.modpackVersion) {
      unwrap(
        await commands.modpackInstall(
          server.modpackProject ?? hit.projectId,
          server.modpackVersion,
          hit.title,
          null,
          "modrinth",
          { name: hit.title, address: server.address },
        ),
      );
      toast.success(t("discover.serverPackStarted", { name: hit.title }));
      return;
    }
    if (!instanceId) return;
    unwrap(await commands.serversAdd(instanceId, hit.title, server.address));
    setJoined((prev) => new Set(prev).add(sameAddress(server.address)));
    toast.success(t("discover.serverAdded", { name: instanceName }));
  };

  const start = async (entry: Entry, kind: InstallKind) => {
    const { hit } = entry;
    if (kind === "modpack") {
      setPackSource({
        kind: "remote",
        source: hit.source,
        projectId: hit.projectId,
        title: hit.title,
      });
      return;
    }
    const key = `${hit.source}:${hit.projectId}`;
    if (kind === "map" && hit.manualOnly) {
      if (hit.pageUrl) {
        openUrl(hit.pageUrl).catch((error: unknown) => reportError(error, "打开地图页面"));
      }
      toast.info(t("discover.mapManualOpened", { name: hit.title }));
      return;
    }
    if (kind === "server") {
      if (resolving.has(key)) return;
      markResolving(key, true);
      try {
        await joinServer(entry);
      } catch (error) {
        reportError(error, "加入服务器");
      } finally {
        markResolving(key, false);
      }
      return;
    }
    if (!instanceId) return;
    // 正在解析、或者已经在等你确认的，再点一下不重复
    if (resolving.has(key) || hasPlan(hit)) return;
    markResolving(key, true);
    try {
      if (kind === "map") {
        unwrap(await commands.mapsInstall(instanceId, hit.projectId, hit.title));
        toast.success(t("discover.mapStarted", { name: hit.title }));
        return;
      }
      const all = unwrap(
        await commands.contentResolve(instanceId, hit.projectId, kind, hit.source),
      );
      const simple =
        all.length === 1 && all[0]?.reason === "requested" && all[0].delivery !== "manual";
      if (simple) await install(instanceId, all);
      else {
        setPlans((prev) => [
          ...prev,
          { items: all, selectDependencies: autoDependencies, instanceId, instanceName },
        ]);
      }
    } catch (error) {
      reportError(error, kind === "map" ? "安装地图" : "解析安装内容");
    } finally {
      markResolving(key, false);
    }
  };

  /** 按钮现在该是什么样 */
  const stateOf = (
    entry: Entry,
    kind: InstallKind,
    installed: Partial<Record<ContentKind, ContentItem[]>>,
  ): InstallState => {
    if (resolving.has(`${entry.hit.source}:${entry.hit.projectId}`)) return { kind: "busy" };
    // 已经解析好、排着等你确认的也算「正在弄」
    if (hasPlan(entry.hit)) return { kind: "busy" };
    if (
      kind === "server" &&
      entry.hit.server &&
      joined.has(sameAddress(entry.hit.server.address))
    ) {
      return { kind: "joined" };
    }
    const job = latestJob(jobs, entry, kind, instanceId);
    if (job?.state === "running") {
      const { done, total } = job.progress;
      return { kind: "running", progress: total ? (done ?? 0) / total : null };
    }
    if (kind === "mod" || kind === "shader" || kind === "resourcePack") {
      const list = installed[kind];
      if (list && findInstalled(entry, list)) return { kind: "done" };
    }
    if (job?.state === "succeeded") return { kind: "done" };
    if (kind === "map" && entry.hit.manualOnly) return { kind: "manual" };
    return { kind: "idle" };
  };

  const dialogs = (
    <>
      <InstallDialog
        plan={plan?.items ?? null}
        selectDependencies={plan?.selectDependencies ?? true}
        instanceName={plan?.instanceName ?? ""}
        busy={installing}
        onConfirm={(items) => {
          if (plan) void install(plan.instanceId, items).then((ok) => ok && nextPlan());
        }}
        onClose={nextPlan}
      />
      {packSource !== undefined && (
        <Suspense fallback={null}>
          <ImportPackDialog source={packSource} onClose={() => setPackSource(null)} />
        </Suspense>
      )}
    </>
  );

  return { start, stateOf, dialogs };
}
