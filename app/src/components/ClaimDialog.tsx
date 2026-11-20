import { openUrl } from "@tauri-apps/plugin-opener";
import {
  CircleAlert,
  CircleCheck,
  ExternalLink,
  FileDown,
  FolderSearch,
  ImageIcon,
  Package,
  Sparkles,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { unwrap } from "@/lib/api";
import { type ClaimEntry, type ClaimSnapshot, type ContentKind, commands } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { settledCount, useClaimStore } from "@/stores/claims";
import { useGameStore } from "@/stores/game";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { HoverHint, Truncated } from "./ui/HoverCard";
import { ProgressBar } from "./ui/ProgressBar";
import { VirtualList } from "./VirtualList";

const KIND_ICON: Record<ContentKind, typeof Package> = {
  mod: Package,
  resourcePack: ImageIcon,
  shader: Sparkles,
};

const formatSize = (bytes: number) =>
  bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`;

function EntryRow({ claim, entry }: { claim: ClaimSnapshot; entry: ClaimEntry }) {
  const { t } = useTranslation();
  const { file, state } = entry;
  const Icon = KIND_ICON[file.kind];

  const open = () => {
    if (!file.pageUrl) return;
    openUrl(file.pageUrl).catch((error: unknown) => reportError(error, "打开下载页"));
  };
  const skip = async (skipped: boolean) => {
    try {
      unwrap(await commands.claimSkip(claim.id, file.fileId, skipped));
    } catch (error) {
      reportError(error, "跳过可选文件");
    }
  };

  const waiting = state.kind === "waiting" || state.kind === "wrong";
  return (
    <div className="flex items-center gap-3 border-line border-b py-2.5 last:border-b-0">
      {file.iconUrl ? (
        <img src={file.iconUrl} alt="" className="size-9 shrink-0 rounded-row" loading="lazy" />
      ) : (
        <div className="grid size-9 shrink-0 place-items-center rounded-row bg-surface-hover">
          <Icon className="size-4 text-fg-muted" aria-hidden />
        </div>
      )}
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium text-sm">
          {file.projectName}
          <span className="ml-2 font-normal text-fg-muted text-xs">
            {t(`kind.${file.kind}`)}
            {!file.required && ` · ${t("claim.optional")}`}
          </span>
        </p>
        <HoverHint className="block truncate text-fg-muted text-xs" hint={file.fileName}>
          {file.displayName} · {formatSize(file.size ?? 0)}
        </HoverHint>
        {state.kind === "wrong" && (
          <p className="truncate text-warning text-xs">{t("claim.wrong", { got: state.got })}</p>
        )}
      </div>
      {state.kind === "claimed" && (
        <span className="flex shrink-0 items-center gap-1 text-accent text-sm">
          <CircleCheck className="size-4" aria-hidden />
          {t("claim.claimed")}
        </span>
      )}
      {state.kind === "skipped" && (
        <>
          <span className="shrink-0 text-fg-muted text-sm">{t("claim.skipped")}</span>
          <Button variant="ghost" onClick={() => void skip(false)}>
            {t("claim.unskip")}
          </Button>
        </>
      )}
      {waiting && !file.required && (
        <Button variant="ghost" onClick={() => void skip(true)}>
          {t("claim.skip")}
        </Button>
      )}
      {waiting &&
        (file.pageUrl ? (
          <Button variant="secondary" onClick={open}>
            <ExternalLink className="size-4" aria-hidden />
            {t("claim.openPage")}
          </Button>
        ) : (
          <span className="shrink-0 text-fg-muted text-xs">
            {t("claim.fileId", { id: file.fileId })}
          </span>
        ))}
    </div>
  );
}

/**
 * 补件面板（招牌特色 7）：作者不允许第三方启动器下载的文件，一个个带用户去精确下载页下载。
 * 下载文件夹里出现、拖进窗口、或者在系统窗口里选的文件，后端按哈希核对后自动认领；
 * 必需的齐了安装就自动继续。面板可以收起，安装在后台一直等着。
 */
export function ClaimDialog() {
  const { t } = useTranslation();
  const claims = useClaimStore((s) => s.claims);
  const shown = useClaimStore((s) => s.shown);
  const dragging = useClaimStore((s) => s.dragging);
  const show = useClaimStore((s) => s.show);
  const cancelJob = useGameStore((s) => s.cancelJob);
  const claim = shown ? claims[shown] : undefined;

  const pick = async () => {
    if (!claim) return;
    try {
      unwrap(await commands.claimPick(claim.id));
    } catch (error) {
      reportError(error, "选择下载好的文件");
    }
  };

  const done = claim ? settledCount(claim) : 0;
  const total = claim?.entries.length ?? 0;
  const required = claim?.entries.filter((e) => e.file.required).length ?? 0;
  // 还要下载的（等着的、下错版本的）里有下载页的
  const pages =
    claim?.entries
      .filter((e) => e.state.kind === "waiting" || e.state.kind === "wrong")
      .map((e) => e.file.pageUrl)
      .filter((url): url is string => !!url) ?? [];

  /** 一次打开全部下载页：CurseForge 的下载页几秒后会自动开始下载，启动器自动认领 */
  const openAll = async () => {
    for (const url of pages) {
      try {
        await openUrl(url);
      } catch (error) {
        reportError(error, "打开下载页");
        return;
      }
    }
  };

  return (
    <Dialog
      open={!!claim}
      onClose={() => show(null)}
      wide
      title={t("claim.title", { name: claim?.title ?? "" })}
      footer={
        claim && (
          <>
            <Button variant="ghost" className="mr-auto" onClick={() => void cancelJob(claim.jobId)}>
              {t("claim.cancelInstall")}
            </Button>
            <Button variant="secondary" onClick={() => show(null)}>
              {t("claim.later")}
            </Button>
          </>
        )
      }
    >
      {claim && (
        <div className="flex flex-col gap-4">
          <p className="text-sm">
            {t("claim.intro", { n: total, required })}
            <span className="text-fg-muted">{t("claim.why")}</span>
          </p>

          <div className="flex flex-col gap-1.5">
            <div className="flex items-center justify-between text-sm">
              <span>{t("claim.progress", { done, total })}</span>
              <span className="text-fg-muted text-xs">{t("claim.autoContinue")}</span>
            </div>
            <ProgressBar value={total ? done / total : 0} label={t("claim.progressLabel")} />
          </div>

          {pages.length > 0 && (
            <div className="flex items-center gap-3">
              <p className="min-w-0 flex-1 text-fg-muted text-xs">{t("claim.openAllHint")}</p>
              <Button variant="primary" onClick={() => void openAll()}>
                <ExternalLink className="size-4" aria-hidden />
                {t("claim.openAll", { n: pages.length })}
              </Button>
            </div>
          )}

          <div
            className={`flex items-center gap-3 rounded-panel border border-dashed p-3 ${
              dragging ? "border-accent bg-accent-soft" : "border-line"
            }`}
          >
            <FileDown
              className={`size-5 shrink-0 ${dragging ? "text-accent" : "text-fg-muted"}`}
              aria-hidden
            />
            <div className="min-w-0 flex-1 text-sm">
              <p>{dragging ? t("claim.dropNow") : t("claim.drop")}</p>
              <Truncated
                as="p"
                className="truncate text-fg-muted text-xs"
                text={
                  claim.watching.length > 0
                    ? t(claim.movesClaimed ? "claim.watchingMove" : "claim.watchingCopy", {
                        path: claim.watching.join("、"),
                      })
                    : t("claim.noWatch")
                }
              />
            </div>
            <Button variant="secondary" onClick={() => void pick()}>
              <FolderSearch className="size-4" aria-hidden />
              {t("claim.pick")}
            </Button>
          </div>

          {claim.notice && (
            <div className="flex items-start gap-2 text-sm">
              <CircleAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden />
              <p className="min-w-0 flex-1">{claim.notice}</p>
            </div>
          )}

          <VirtualList
            items={claim.entries}
            estimate={64}
            getKey={(entry) => String(entry.file.fileId)}
            render={(entry) => <EntryRow claim={claim} entry={entry} />}
            className="max-h-80 rounded-panel border border-line px-3"
          />
        </div>
      )}
    </Dialog>
  );
}
