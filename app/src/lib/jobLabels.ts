import type { TFunction } from "i18next";
import type { JobSnapshot } from "./bindings";
import { getUiLanguage } from "./i18n";

// 旧的下载历史也只保存后端文案。按任务键和已知模板翻译动作，
// 保留用户和内容平台给出的名字，包括可能碰巧像「等 2 项」的后缀。
const TITLE_PATTERNS = new Map<string, readonly (readonly [RegExp, string])[]>([
  [
    "backup",
    [
      [/^备份世界「(.+)」$/s, "backup"],
      [/^恢复世界「(.+)」$/s, "restore"],
    ],
  ],
  [
    "content",
    [
      [/^安装 (.+)$/s, "install"],
      [/^更新 (.+)$/s, "update"],
    ],
  ],
  ["instance-copy", [[/^复制实例 (.+)$/s, "copy"]]],
  [
    "instance-create",
    [
      [/^新建实例 (.+)$/s, "create"],
      [/^导入实例 (.+)$/s, "importInstance"],
    ],
  ],
  ["instance-export", [[/^导出整合包 (.+)$/s, "exportPack"]]],
  ["java", [[/^下载 Java (\d+)$/, "java"]]],
  ["launch", [[/^准备启动 (.+)$/s, "launch"]]],
  ["map-install", [[/^安装地图 (.+)$/s, "map"]]],
  [
    "modpack-import",
    [
      [/^导入整合包 (.+)$/s, "importPack"],
      [/^安装整合包 (.+)$/s, "installPack"],
    ],
  ],
  ["repair", [[/^修复游戏文件 · (.+)$/s, "repair"]]],
]);

export function jobTitle(job: Pick<JobSnapshot, "key" | "title">, t: TFunction): string {
  if (getUiLanguage() === "zh-CN") return job.title;
  const [kind = ""] = job.key.split(":", 1);
  if (kind === "drop") {
    const localFiles = /^装进 (.+)：(\d+) 个文件$/s.exec(job.title);
    if (localFiles) {
      return t("tasks.jobTitle.drop", { name: localFiles[1], count: Number(localFiles[2]) });
    }
  }
  for (const [pattern, key] of TITLE_PATTERNS.get(kind) ?? []) {
    const match = pattern.exec(job.title);
    if (match) return t(`tasks.jobTitle.${key}`, { name: match[1] });
  }
  return job.title;
}

const PROGRESS_LABELS = new Map([
  ["下载", "download"],
  ["下载资源索引", "assetIndex"],
  ["补全游戏文件", "gameFiles"],
  ["补齐文件", "gameFiles"],
  ["解压本地库", "natives"],
  ["下载原版客户端", "vanillaClient"],
  ["下载加载器安装器", "loaderInstaller"],
  ["下载加载器依赖", "loaderLibraries"],
  ["检查版本文件", "versionFiles"],
  ["校验并补全游戏文件", "repairFiles"],
  ["核验 Java 版游玩权限", "javaEntitlement"],
  ["检查游戏文件", "checkGameFiles"],
  ["下载 Java", "java"],
  ["放进实例", "installContent"],
  ["下载新版本", "newVersion"],
  ["下载地图", "map"],
  ["下载整合包", "modpack"],
  ["检查整合包", "checkPack"],
  ["下载整合包文件", "modpackFiles"],
  ["解压配置文件", "overrides"],
  ["查 Modrinth 上的文件", "modrinthFiles"],
  ["打包", "package"],
  ["复制文件", "copyFiles"],
  ["进世界前先备份一份", "backupBeforePlay"],
  ["排队中：等同一个游戏目录前面的事做完", "waitingTurn"],
]);

function progressStage(text: string, t: TFunction): string | undefined {
  const key = PROGRESS_LABELS.get(text);
  if (key) return t(`tasks.jobProgress.${key}`);
  const java = /^下载 Java (\d+)$/.exec(text);
  return java ? t("tasks.jobProgress.javaVersion", { major: java[1] }) : undefined;
}

/** 翻译当前进度和历史步骤的已知文案，外部加载器步骤名按原样插入。 */
export function jobProgressLabel(text: string | null, t: TFunction): string | null {
  if (text == null || getUiLanguage() === "zh-CN") return text;

  const processor = /^安装加载器 (\d+)\/(\d+)：(.*)$/s.exec(text);
  if (processor) {
    const [, done, total, task] = processor;
    return t("tasks.jobProgress.loaderProcessor", { done, total, task });
  }
  const manual = /^等你补齐 (\d+) 个文件（作者不允许第三方启动器下载）$/.exec(text);
  if (manual) return t("tasks.jobProgress.manualFiles", { count: Number(manual[1]) });

  const counted = /^(.+) (\d+)\/(\d+)$/.exec(text);
  if (counted?.[1]) {
    const label = progressStage(counted[1], t);
    if (label)
      return t("tasks.jobProgress.counted", { label, done: counted[2], total: counted[3] });
  }
  return progressStage(text, t) ?? text;
}
