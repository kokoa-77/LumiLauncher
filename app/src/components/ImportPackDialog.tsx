import { CircleAlert, CircleCheck, Download, FileDown, Package } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  type ContentSource,
  commands,
  type PackChoice,
  type PackReadiness,
  type PackVersion,
} from "@/lib/bindings";
import { logError, reportError, userMessage } from "@/lib/report";
import { useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Field, TextInput } from "./ui/Field";
import { HoverHint, Truncated } from "./ui/HoverCard";
import { Select } from "./ui/Select";

/** 整合包从哪来：本地文件（后端弹系统选择窗口）、拖进窗口的文件（一次性编号），或者 Modrinth / CurseForge 上的一个项目 */
export type PackSourceRef =
  | { kind: "file" }
  | { kind: "dropped"; token: string }
  | { kind: "remote"; source: ContentSource; projectId: string; title: string };

const MB = 1024 * 1024;
const formatMb = (bytes: number) => `${(bytes / MB).toFixed(1)} MB`;

type RowState = "ok" | "auto";

function ReadyRow({ state, label, value }: { state: RowState; label: string; value: string }) {
  const Icon = state === "ok" ? CircleCheck : Download;
  const tone = state === "ok" ? "text-accent" : "text-fg-muted";
  return (
    <li className="flex items-center gap-2">
      <Icon className={`size-4 shrink-0 ${tone}`} aria-hidden />
      <span className="shrink-0">{label}</span>
      <Truncated className="truncate text-fg-muted" text={value} />
    </li>
  );
}

/** 准备情况：游戏本体、加载器、Java。`loaderVersion` 为空表示还不知道具体版本（在线安装时） */
function Readiness({
  readiness: r,
  gameVersion,
  loader,
}: {
  readiness: PackReadiness;
  gameVersion: string;
  loader: { known: true; version: string } | { known: false } | null;
}) {
  const { t } = useTranslation();
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-fg-muted text-sm">{t("import.ready")}</h3>
      <ul className="flex flex-col gap-1 text-sm">
        <ReadyRow
          state={r.gameInstalled ? "ok" : "auto"}
          label={`Minecraft ${gameVersion}`}
          value={r.gameInstalled ? t("import.installed") : t("import.autoDownload")}
        />
        {loader?.known && (
          <ReadyRow
            state={r.loaderInstalled ? "ok" : "auto"}
            label={`NeoForge ${loader.version}`}
            value={r.loaderInstalled ? t("import.installed") : t("import.autoInstall")}
          />
        )}
        {loader && !loader.known && (
          <ReadyRow state="auto" label="NeoForge" value={t("import.loaderByPack")} />
        )}
        <ReadyRow
          state={r.javaFound ? "ok" : "auto"}
          label={`Java ${r.javaRequired ?? ""}`.trim()}
          value={
            r.javaRequired === null
              ? t("import.javaLater")
              : r.javaFound
                ? t("import.javaFound", { version: r.javaFound })
                : t("import.javaAuto")
          }
        />
      </ul>
    </section>
  );
}

/** 准备情况查完就能开始：缺的游戏本体、加载器、Java 都会自动装 */
const canStart = (r: PackReadiness | null | undefined) => !!r;

/**
 * 导入 / 安装整合包。
 * - 本地文件：打开时由后端弹出系统选择窗口（界面拿不到路径），检查完显示预览，确认后在后台导入
 * - Modrinth：马上显示版本和准备情况（不用先下载）；点「安装」就交给后台任务，
 *   下载、检查、安装一气呵成，可以随便切页面，进度在「实例」页和任务中心
 *
 * 缺 Java 时不让开始（装好也启动不了）。
 */
export function ImportPackDialog({
  source,
  onClose,
}: {
  source: PackSourceRef | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  // 本地导入
  const [choice, setChoice] = useState<PackChoice | null>(null);
  // 在线安装
  const [versions, setVersions] = useState<PackVersion[] | null>(null);
  const [versionId, setVersionId] = useState("");
  const [readiness, setReadiness] = useState<PackReadiness | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  // 父组件每次渲染都会传新的 onClose；放进依赖会让选择窗口反复弹出，所以存在引用里
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  /** 本地导入还没交给后台的检查结果（关掉时告诉后端丢掉） */
  const tokenRef = useRef<string | null>(null);
  /** 每次重新打开都加一，晚到的旧结果直接丢掉 */
  const requestRef = useRef(0);
  // 拖放令牌只能消费一次。首屏挂载和父组件重渲染会重跑 effect。
  const consumedDrop = useRef<string | null>(null);

  const discard = (token: string | null) => {
    if (!token) return;
    commands.modpackDiscard(token).catch((error: unknown) => logError(error, "丢掉整合包检查结果"));
  };

  const loadReadiness = async (version: PackVersion | undefined) => {
    const current = requestRef.current;
    setReadiness(null);
    const gameVersion = version?.gameVersions[0];
    if (!gameVersion) return;
    try {
      const result = unwrap(await commands.modpackReadiness(gameVersion));
      if (current === requestRef.current) setReadiness(result);
    } catch (error) {
      if (current !== requestRef.current) return;
      logError(error, "检查整合包准备情况");
      setFailure(userMessage(error));
    }
  };

  const loadVersions = async (projectId: string, from: ContentSource) => {
    const current = requestRef.current;
    try {
      const list = unwrap(await commands.modpackVersions(projectId, from));
      if (current !== requestRef.current) return;
      setVersions(list);
      const preferred = list.find((v) => v.recommended) ?? list[0];
      if (!preferred) {
        setFailure(t("import.noVersion"));
        return;
      }
      setVersionId(preferred.id);
      await loadReadiness(preferred);
    } catch (error) {
      if (current !== requestRef.current) return;
      logError(error, "读取整合包版本");
      setFailure(userMessage(error));
    }
  };

  useEffect(() => {
    if (!source) {
      consumedDrop.current = null;
      return;
    }
    if (source.kind === "dropped") {
      if (consumedDrop.current === source.token) return;
      consumedDrop.current = source.token;
    } else {
      consumedDrop.current = null;
    }
    requestRef.current += 1;
    const current = requestRef.current;
    setChoice(null);
    setVersions(null);
    setVersionId("");
    setReadiness(null);
    setFailure(null);
    if (source.kind === "remote") {
      setName(source.title);
      void loadVersions(source.projectId, source.source);
      return;
    }
    const picking =
      source.kind === "dropped"
        ? commands.dropsModpack(source.token).then(unwrap)
        : commands.modpackPick().then(unwrap);
    picking
      .then((picked) => {
        if (current !== requestRef.current) {
          if (picked) discard(picked.token);
          return;
        }
        if (!picked) {
          closeRef.current();
          return;
        }
        tokenRef.current = picked.token;
        setChoice(picked);
        setName(picked.preview.name);
      })
      .catch((error: unknown) => {
        if (current !== requestRef.current) return;
        reportError(error, "选择整合包");
        closeRef.current();
      });
  }, [source]);

  const close = () => {
    requestRef.current += 1;
    discard(tokenRef.current);
    tokenRef.current = null;
    setChoice(null);
    onClose();
  };

  const remote = source?.kind === "remote" ? source : null;
  const version = versions?.find((v) => v.id === versionId);

  const start = async () => {
    setBusy(true);
    const finalName = name.trim() || null;
    try {
      if (remote) {
        if (!versionId) return;
        unwrap(
          await commands.modpackInstall(
            remote.projectId,
            versionId,
            remote.title,
            finalName,
            remote.source,
            null,
          ),
        );
        toast.success(t("import.started", { name: finalName ?? remote.title }));
      } else {
        if (!choice) return;
        const jobId = unwrap(await commands.modpackImport(choice.token, finalName));
        // 检查结果已经交给后台任务，不用再丢
        tokenRef.current = null;
        // 拖进来的整合包：和拖进来的模组一样，自动打开任务中心看进度
        if (source?.kind === "dropped") useGameStore.getState().showTasksFor(jobId);
        toast.success(t("import.started", { name: finalName ?? choice.preview.name }));
      }
      setChoice(null);
      onClose();
    } catch (error) {
      reportError(error, remote ? "安装整合包" : "导入整合包");
    } finally {
      setBusy(false);
    }
  };

  const p = choice?.preview;
  const ready = remote ? readiness : choice?.readiness;

  return (
    <Dialog
      open={source !== null && (remote !== null || choice !== null)}
      onClose={close}
      wide
      title={remote ? t("import.installTitle", { name: remote.title }) : t("import.title")}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={busy || !canStart(ready) || (remote ? !versionId : !choice)}
            onClick={() => void start()}
          >
            {remote ? t("import.install") : t("import.confirm")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {failure && (
          <div className="flex items-start gap-3 rounded-panel border border-line p-3">
            <CircleAlert className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
            <p className="min-w-0 flex-1 text-sm">{failure}</p>
            {remote && (
              <Button
                variant="secondary"
                onClick={() => {
                  setFailure(null);
                  void (versions
                    ? loadReadiness(version)
                    : loadVersions(remote.projectId, remote.source));
                }}
              >
                {t("import.retry")}
              </Button>
            )}
          </div>
        )}

        {remote && !versions && !failure && (
          <p className="py-2 text-fg-muted text-sm">{t("common.loading")}</p>
        )}

        {remote && versions && versions.length > 0 && (
          <>
            <Field label={t("import.version")}>
              <Select
                label={t("import.version")}
                value={versionId}
                disabled={busy}
                onChange={(id) => {
                  setVersionId(id);
                  setFailure(null);
                  void loadReadiness(versions.find((v) => v.id === id));
                }}
                options={versions.map((v) => ({
                  value: v.id,
                  label: [
                    v.versionNumber,
                    v.gameVersions.join(" / "),
                    v.release ? null : t("import.beta"),
                  ]
                    .filter(Boolean)
                    .join(" · "),
                }))}
              />
            </Field>
            {version && (
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
                <dt className="text-fg-muted">{t("import.game")}</dt>
                <dd>Minecraft {version.gameVersions.join(" / ")} · NeoForge</dd>
                <dt className="text-fg-muted">{t("import.packFile")}</dt>
                <dd>{t("import.packFileValue", { size: formatMb(version.size ?? 0) })}</dd>
              </dl>
            )}
            {readiness && version && (
              <Readiness
                readiness={readiness}
                gameVersion={version.gameVersions[0] ?? ""}
                loader={{ known: false }}
              />
            )}
          </>
        )}

        {p && choice && (
          <>
            <div className="flex items-start gap-3">
              <div className="grid size-11 shrink-0 place-items-center rounded-row bg-accent-soft text-accent">
                <Package className="size-5" aria-hidden />
              </div>
              <div className="min-w-0">
                <p className="font-medium">
                  {p.name} <span className="font-normal text-fg-muted text-sm">{p.version}</span>
                </p>
                {p.summary && <p className="line-clamp-2 text-fg-muted text-sm">{p.summary}</p>}
              </div>
            </div>
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
              <dt className="text-fg-muted">{t("import.format")}</dt>
              <dd>{t(`import.formatValue.${p.format}`)}</dd>
              <dt className="text-fg-muted">{t("import.game")}</dt>
              <dd>
                Minecraft {p.gameVersion} ·{" "}
                {p.loader ? `NeoForge ${p.loader.version}` : t("create.vanilla")}
              </dd>
              <dt className="text-fg-muted">{t("import.files")}</dt>
              <dd>
                {t("import.filesValue", {
                  n: p.fileCount,
                  size: formatMb(p.downloadBytes ?? 0),
                  overrides: p.overrideCount,
                })}
              </dd>
              {p.optionalFiles.length > 0 && (
                <>
                  <dt className="text-fg-muted">{t("import.optional")}</dt>
                  <dd>
                    <HoverHint hint={p.optionalFiles.join("、")}>
                      {t(
                        p.format === "curseForge"
                          ? "import.optionalValueDisabled"
                          : "import.optionalValue",
                        { n: p.optionalFiles.length },
                      )}
                    </HoverHint>
                  </dd>
                </>
              )}
              <dt className="text-fg-muted">{t("import.safety")}</dt>
              <dd className="text-accent">{t("import.safetyOk")}</dd>
            </dl>
            {p.fromModrinth > 0 && (
              <div className="flex items-start gap-3 rounded-panel border border-line p-3">
                <CircleCheck className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
                <p className="min-w-0 flex-1 text-sm">
                  {t("import.fromModrinth", { n: p.fromModrinth })}
                </p>
              </div>
            )}
            {p.manualFiles.length > 0 && (
              <div className="flex items-start gap-3 rounded-panel border border-line p-3">
                <FileDown className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
                <div className="min-w-0 flex-1 text-sm">
                  <p>{t("import.manual", { n: p.manualFiles.length })}</p>
                  <Truncated
                    as="p"
                    className="line-clamp-2 text-fg-muted text-xs"
                    text={p.manualFiles.map((f) => f.projectName).join("、")}
                  />
                </div>
              </div>
            )}
            <Readiness
              readiness={choice.readiness}
              gameVersion={p.gameVersion}
              loader={p.loader ? { known: true, version: p.loader.version } : null}
            />
          </>
        )}

        {(remote ? versions && versions.length > 0 : choice) && (
          <Field label={t("import.name")} hint={t("import.nameHint")}>
            <TextInput
              value={name}
              maxLength={48}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
        )}

        {remote && versions && versions.length > 0 && (
          <p className="text-fg-muted text-xs">{t("import.backgroundHint")}</p>
        )}
      </div>
    </Dialog>
  );
}
