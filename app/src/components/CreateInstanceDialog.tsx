import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import { commands, type LoaderKind, type LoaderVersion } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { Button } from "./ui/Button";
import { Dialog } from "./ui/Dialog";
import { Field, TextInput } from "./ui/Field";
import { Segmented } from "./ui/Segmented";
import { Select } from "./ui/Select";

type LoaderChoice = "vanilla" | LoaderKind;
const LABELS: Record<LoaderKind, string> = {
  neoforge: "NeoForge",
  fabric: "Fabric",
  forge: "Forge",
  quilt: "Quilt",
};
interface CatalogResult {
  key: string;
  versions: LoaderVersion[] | "unsupported" | "error";
}

/** 新建实例向导：名字、游戏版本、加载器。确认后在后台安装，进度在任务中心。 */
export function CreateInstanceDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const [versions, setVersions] = useState<string[]>([]);
  const [pickedGameVersion, setPickedGameVersion] = useState<string | null>(null);
  const gameVersion = pickedGameVersion ?? versions[0] ?? "";
  const [loader, setLoader] = useState<LoaderChoice>("neoforge");
  const [catalog, setCatalog] = useState<CatalogResult | null>(null);
  const [picked, setPicked] = useState<{ key: string; version: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  /** 手动改过的名字；null 表示用自动名字（跟着版本变） */
  const [customName, setCustomName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open || versions.length > 0) return;
    let current = true;
    commands
      .catalogGameVersions()
      .then(unwrap)
      .then((list) => {
        if (current) setVersions(list);
      })
      .catch((error: unknown) => {
        if (current) reportError(error, "读取游戏版本列表");
      });
    return () => {
      current = false;
    };
  }, [open, versions.length]);

  const key = `${gameVersion}:${loader}:${attempt}`;
  useEffect(() => {
    if (!open || !gameVersion || loader === "vanilla") return;
    let current = true;
    commands
      .catalogLoaderVersions(gameVersion, loader)
      .then(unwrap)
      .then((list) => {
        if (current) setCatalog({ key, versions: list ?? "unsupported" });
      })
      .catch((error: unknown) => {
        if (!current) return;
        setCatalog({ key, versions: "error" });
        reportError(error, "读取加载器版本列表");
      });
    return () => {
      current = false;
    };
  }, [open, gameVersion, loader, key]);

  // 按请求标识匹配，切换游戏或加载器时不会拿上一组的版本去创建。
  const catalogVersions = catalog?.key === key ? catalog.versions : undefined;
  const loaderVersions = Array.isArray(catalogVersions) ? catalogVersions : undefined;
  const pickedVersion = picked?.key === key ? picked.version : undefined;
  const loaderVersion = loaderVersions?.some((v) => v.version === pickedVersion)
    ? (pickedVersion ?? "")
    : ((loaderVersions?.find((v) => v.stable) ?? loaderVersions?.[0])?.version ?? "");
  const isVanilla = loader === "vanilla";
  const loaderLabel = isVanilla ? "" : LABELS[loader];
  const autoName =
    loaderLabel && gameVersion
      ? `${gameVersion}-${loaderLabel}${loaderVersion ? ` ${loaderVersion}` : ""}`
      : gameVersion;
  const name = customName ?? autoName;
  const canCreate =
    !busy && name.trim() !== "" && gameVersion !== "" && (isVanilla || loaderVersion !== "");

  const close = () => {
    setPickedGameVersion(null);
    setPicked(null);
    onClose();
  };

  const create = async () => {
    setBusy(true);
    try {
      unwrap(
        await commands.instancesCreate(
          name.trim(),
          gameVersion,
          isVanilla ? null : { kind: loader, version: loaderVersion },
        ),
      );
      toast.success(t("create.started", { name: name.trim() }));
      close();
      setCustomName(null);
    } catch (error) {
      reportError(error, "新建实例");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      title={t("create.title")}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={!canCreate} onClick={() => void create()}>
            {t("create.confirm")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("create.name")} hint={t("create.nameHint")}>
          <TextInput
            value={name}
            placeholder={autoName}
            maxLength={48}
            onChange={(event) => setCustomName(event.target.value)}
            // 清空后离开输入框：恢复自动名字
            onBlur={() => {
              if (customName?.trim() === "") setCustomName(null);
            }}
          />
        </Field>
        <Field label={t("create.gameVersion")}>
          <Select
            label={t("create.gameVersion")}
            value={gameVersion}
            disabled={versions.length === 0}
            placeholder={t("common.loading")}
            onChange={(version) => {
              setPicked(null);
              setCatalog(null);
              setPickedGameVersion(version);
            }}
            options={versions.map((v) => ({ value: v, label: v }))}
          />
        </Field>
        <div className="flex flex-col gap-1.5">
          <span className="font-medium text-sm">{t("create.loader")}</span>
          <Segmented
            label={t("create.loader")}
            value={loader}
            onChange={(kind) => {
              setPicked(null);
              setCatalog(null);
              setLoader(kind);
            }}
            options={[
              { value: "neoforge", label: LABELS.neoforge },
              { value: "fabric", label: LABELS.fabric },
              { value: "forge", label: LABELS.forge },
              { value: "quilt", label: LABELS.quilt },
              { value: "vanilla", label: t("create.vanilla") },
            ]}
          />
        </div>
        {!isVanilla && (
          <Field
            label={t("create.loaderVersion", { loader: loaderLabel })}
            hint={
              catalogVersions === "unsupported"
                ? t("create.unsupportedLoader", { loader: loaderLabel, version: gameVersion })
                : loaderVersions?.length === 0
                  ? t("create.noLoader", { loader: loaderLabel, version: gameVersion })
                  : loaderVersions?.length
                    ? t("create.loaderHint")
                    : undefined
            }
          >
            {catalogVersions === "error" ? (
              <Button onClick={() => setAttempt((n) => n + 1)}>{t("create.retryCatalog")}</Button>
            ) : catalogVersions === "unsupported" ? null : (
              <Select
                label={t("create.loaderVersion", { loader: loaderLabel })}
                value={loaderVersion}
                disabled={!loaderVersions?.length}
                placeholder={loaderVersions ? "" : t("common.loading")}
                onChange={(version) => setPicked({ key, version })}
                options={(loaderVersions ?? []).map((v) => ({
                  value: v.version,
                  label: v.stable ? v.version : t("create.beta", { version: v.version }),
                }))}
              />
            )}
          </Field>
        )}
      </div>
    </Dialog>
  );
}
