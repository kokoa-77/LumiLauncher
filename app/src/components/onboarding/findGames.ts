import { unwrap } from "@/lib/api";
import { commands, type JobSnapshot } from "@/lib/bindings";
import i18n from "@/lib/i18n";
import { logError, userMessage } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import type { Found } from "./useFound";

type FindItem = { kind: "link" | "import"; id: string; name: string };
type FindOutcome = FindItem & { error: string | null };

export interface FindSelection {
  // 找到的目录默认勾上，记住用户主动取消的项。
  excludedLinks: Set<string>;
  imports: Set<string>;
  outcomes: Record<string, FindOutcome>;
  retry: FindItem[];
}

export function createFindSelection(): FindSelection {
  return { excludedLinks: new Set(), imports: new Set(), outcomes: {}, retry: [] };
}

export function findKey(kind: FindItem["kind"], id: string): string {
  return `${kind}:${id}`;
}

/** 已有实例编号就说明文件复制并登记成功。本体和加载器仍可在后台安装。 */
function waitForImport(jobId: string): Promise<string | null> {
  const store = useGameStore;
  return new Promise((resolve) => {
    const check = (job: JobSnapshot | undefined) => {
      if (!job || (job.state === "running" && !job.instanceId)) return;
      stop();
      clearInterval(poll);
      if (job.instanceId || job.state === "succeeded") {
        resolve(null);
      } else {
        resolve(job.error ?? i18n.t("onboarding.find.importStopped"));
      }
    };
    const stop = store.subscribe((state, before) => {
      if (state.jobs[jobId] !== before.jobs[jobId]) check(state.jobs[jobId]);
    });
    // 任务事件可能在初始化或队列积压时错过，用任务快照兜住。
    const poll = setInterval(() => {
      void commands
        .jobsList()
        .then((jobs) => check(jobs.find((job) => job.id === jobId)))
        .catch((error: unknown) => logError(error, "确认引导导入任务进度"));
    }, 5_000);
    // 短任务可能在命令返回编号之前就已收到事件。
    check(store.getState().jobs[jobId]);
  });
}

function selectedItems(found: Found, selection: FindSelection): FindItem[] {
  const items: FindItem[] = [];
  for (const root of found.roots) {
    if (!selection.excludedLinks.has(root.id)) {
      items.push({ kind: "link", id: root.id, name: root.name });
    }
  }
  for (const item of found.foreign) {
    if (item.supported && selection.imports.has(item.id)) {
      items.push({ kind: "import", id: item.id, name: item.name });
    }
  }
  return items.filter((item) => selection.outcomes[findKey(item.kind, item.id)]?.error !== null);
}

export async function runFindSelection(
  found: Found,
  selection: FindSelection,
): Promise<FindSelection> {
  const tasks = selection.retry.length > 0 ? selection.retry : selectedItems(found, selection);
  const outcomes = { ...selection.outcomes };
  const retry: FindItem[] = [];
  for (const task of tasks) {
    let error: string | null = null;
    try {
      if (task.kind === "link") {
        unwrap(await commands.linkedRootsAdd(task.id));
      } else {
        error = await waitForImport(unwrap(await commands.foreignImport(task.id)));
      }
    } catch (cause) {
      logError(cause, task.kind === "link" ? "挂上别的启动器的目录" : "导入别的启动器的实例");
      error = userMessage(cause);
    }
    outcomes[findKey(task.kind, task.id)] = { ...task, error };
    if (error !== null) retry.push(task);
  }
  if (tasks.length > 0) {
    const { refreshInstances, refreshRecentWorlds } = useGameStore.getState();
    await Promise.all([refreshInstances(), refreshRecentWorlds()]);
  }
  return { ...selection, outcomes, retry };
}
