import { Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { LightSpinner } from "@/components/ui/LightSpinner";
import { unwrap } from "@/lib/api";
import {
  type ClaimFolderInfo,
  commands,
  type DnsProbe,
  type EndpointProbe,
  type Platform,
  type ProxyMode,
  type ProxyScheme,
  type Settings,
} from "@/lib/bindings";
import { logError, reportError } from "@/lib/report";
import { Button } from "../ui/Button";
import { TextInput } from "../ui/Field";
import { Truncated } from "../ui/HoverCard";
import { Segmented } from "../ui/Segmented";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";
import { Group, Row, type UpdateSettings } from "./parts";

const CONCURRENCY_CHOICES = ["8", "16", "32", "64"] as const;

function parsePort(text: string, fallback: number): number {
  const parsed = Number.parseInt(text.trim(), 10);
  if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) return fallback;
  return parsed;
}

/**
 * 设置 →「内容与下载」：
 * - 内容：首选平台、自动装前置
 * - 下载：最大并发数、网络代理（跟随系统 / 直连 / 自定义 HTTP·SOCKS5）
 * - 网络诊断：DoH 解析与测试、核心官方端点一键连通性检测与复制报告
 * - 补件：盯着哪个文件夹、认领后移走原文件
 */
export function DownloadSettings({
  settings,
  update,
}: {
  settings: Settings;
  update: UpdateSettings;
}) {
  const { t, i18n } = useTranslation();
  const isZh = i18n.language.toLowerCase().startsWith("zh");
  const download = settings.download;
  const [dns, setDns] = useState<DnsProbe[] | null>(null);
  const [dnsBusy, setDnsBusy] = useState(false);
  const [probes, setProbes] = useState<EndpointProbe[] | null>(null);
  const [probesBusy, setProbesBusy] = useState(false);
  const [folder, setFolder] = useState<ClaimFolderInfo | null>(null);

  // 自定义代理草稿（切到自定义但尚未填写主机时，先在本地编辑，点「应用代理」再保存生效）
  const [proxyModeDraft, setProxyModeDraft] = useState<ProxyMode>(download.proxy.mode);
  const [proxyScheme, setProxyScheme] = useState<ProxyScheme>(download.proxy.scheme);
  const [proxyHost, setProxyHost] = useState(download.proxy.host);
  const [proxyPort, setProxyPort] = useState(String(download.proxy.port || 7890));

  useEffect(() => {
    setProxyModeDraft(download.proxy.mode);
    setProxyScheme(download.proxy.scheme);
    setProxyHost(download.proxy.host);
    setProxyPort(String(download.proxy.port || 7890));
  }, [download.proxy.mode, download.proxy.scheme, download.proxy.host, download.proxy.port]);

  useEffect(() => {
    commands
      .claimFolderInfo()
      .then(setFolder)
      .catch((error: unknown) => logError(error, "读取补件文件夹"));
  }, []);

  const set = (patch: Partial<Settings["download"]>) =>
    update((s) => ({ ...s, download: { ...s.download, ...patch } }));

  const onProxyMode = (mode: ProxyMode) => {
    setProxyModeDraft(mode);
    if (mode !== "custom") {
      set({ proxy: { ...download.proxy, mode } });
      toast.success(t("settings.proxySaved"));
    }
  };

  const applyCustomProxy = () => {
    const host = proxyHost.trim();
    if (!host) {
      toast.error(t("settings.proxyHostRequired"));
      return;
    }
    const port = parsePort(proxyPort, download.proxy.port || 7890);
    setProxyPort(String(port));
    set({
      proxy: {
        mode: "custom",
        scheme: proxyScheme,
        host,
        port,
      },
    });
    toast.success(t("settings.proxySaved"));
  };

  const testDns = async () => {
    setDnsBusy(true);
    try {
      setDns(unwrap(await commands.netDohTest()));
    } catch (error) {
      reportError(error, "测试 DoH");
    } finally {
      setDnsBusy(false);
    }
  };

  const runDiagnostics = async () => {
    setProbesBusy(true);
    try {
      setProbes(unwrap(await commands.netDiagnostics()));
    } catch (error) {
      reportError(error, "网络连通性检测");
    } finally {
      setProbesBusy(false);
    }
  };

  const copyDiagnostics = async () => {
    if (!probes) return;
    const lines = [
      `Proxy: ${download.proxy.mode}${download.proxy.mode === "custom" ? ` (${download.proxy.scheme})` : ""} | DoH: ${download.dohEnabled ? "on" : "off"}`,
      ...probes.map((probe) => {
        const status = t(`settings.netDiagStatus.${probe.status}`);
        const latency = probe.latencyMs != null ? `${Math.round(probe.latencyMs)} ms` : "-";
        const extra = probe.httpStatus != null ? `HTTP ${probe.httpStatus}` : (probe.error ?? "");
        return `- ${t(`settings.netDiagEndpoint.${probe.id}`)} (${probe.host}): ${status} · ${latency}${extra ? ` · ${extra}` : ""}`;
      }),
    ];
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      toast.success(t("settings.netDiagCopied"));
    } catch (error) {
      reportError(error, "复制网络诊断结果");
    }
  };

  const chooseFolder = async () => {
    try {
      const chosen = unwrap(await commands.claimFolderChoose());
      if (chosen) setFolder(chosen);
    } catch (error) {
      reportError(error, "选择补件文件夹");
    }
  };

  const resetFolder = async () => {
    try {
      setFolder(unwrap(await commands.claimFolderReset()));
    } catch (error) {
      reportError(error, "改回系统的下载文件夹");
    }
  };

  const concurrency = String(download.maxConcurrency);
  const concurrencyOptions = CONCURRENCY_CHOICES.includes(
    concurrency as (typeof CONCURRENCY_CHOICES)[number],
  )
    ? CONCURRENCY_CHOICES
    : ([...CONCURRENCY_CHOICES, concurrency] as string[]);

  return (
    <>
      <Group title={t("settings.contentGroup")}>
        <Row
          label={t("settings.preferredPlatform")}
          hint={t("settings.preferredPlatformHint")}
          control={
            <Segmented<Platform>
              label={t("settings.preferredPlatform")}
              value={download.preferredPlatform}
              options={[
                { value: "modrinth", label: "Modrinth" },
                { value: "curseForge", label: "CurseForge" },
              ]}
              onChange={(preferredPlatform) => set({ preferredPlatform })}
            />
          }
        />
        <Row
          label={t("settings.autoDependencies")}
          hint={t("settings.autoDependenciesHint")}
          control={
            <Switch
              label={t("settings.autoDependencies")}
              checked={download.autoDependencies}
              onChange={(autoDependencies) => set({ autoDependencies })}
            />
          }
        />
      </Group>
      <Group title={t("settings.downloadGroup")}>
        <Row
          label={t("settings.maxConcurrency")}
          hint={t("settings.maxConcurrencyHint")}
          control={
            <Segmented<string>
              label={t("settings.maxConcurrency")}
              value={concurrency}
              options={concurrencyOptions.map((value) => ({ value, label: value }))}
              onChange={(value) => set({ maxConcurrency: Number(value) })}
            />
          }
        />
        <Row
          label={t("settings.proxy")}
          hint={t("settings.proxyHint")}
          control={
            <Segmented<ProxyMode>
              label={t("settings.proxy")}
              value={proxyModeDraft}
              options={[
                { value: "system", label: t("settings.proxyMode.system") },
                { value: "direct", label: t("settings.proxyMode.direct") },
                { value: "custom", label: t("settings.proxyMode.custom") },
              ]}
              onChange={onProxyMode}
            />
          }
        />
        {proxyModeDraft === "custom" && (
          <div className="flex flex-wrap items-center gap-2 px-4 py-3">
            <div className="w-28 shrink-0">
              <Select
                label={t("settings.proxyScheme")}
                value={proxyScheme}
                onChange={(value) => setProxyScheme(value as ProxyScheme)}
                options={[
                  { value: "http", label: "HTTP" },
                  { value: "socks5", label: "SOCKS5" },
                ]}
              />
            </div>
            <div className="min-w-40 flex-1">
              <TextInput
                value={proxyHost}
                onChange={(event) => setProxyHost(event.target.value)}
                placeholder={t("settings.proxyHostPlaceholder")}
                aria-label={t("settings.proxyHost")}
                onKeyDown={(event) => {
                  if (event.key === "Enter") applyCustomProxy();
                }}
              />
            </div>
            <div className="w-24 shrink-0">
              <TextInput
                value={proxyPort}
                onChange={(event) => setProxyPort(event.target.value)}
                inputMode="numeric"
                className="px-2 text-center tabular-nums"
                aria-label={t("settings.proxyPort")}
                onKeyDown={(event) => {
                  if (event.key === "Enter") applyCustomProxy();
                }}
              />
            </div>
            <Button variant="secondary" onClick={applyCustomProxy}>
              {t("settings.proxyApply")}
            </Button>
          </div>
        )}
      </Group>
      <Group title={t("settings.netDiagGroup")}>
        <Row
          label={t("settings.doh")}
          hint={t("settings.dohHint")}
          control={
            <div className="flex items-center gap-3">
              <Button variant="secondary" disabled={dnsBusy} onClick={() => void testDns()}>
                {dnsBusy && <LightSpinner className="size-4" />}
                {dnsBusy ? t("settings.dohTesting") : t("settings.dohTest")}
              </Button>
              <Switch
                label={t("settings.doh")}
                checked={download.dohEnabled}
                onChange={(dohEnabled) => set({ dohEnabled })}
              />
            </div>
          }
        />
        {dns && (
          <ul className="flex flex-col gap-1 px-4 py-3 text-xs">
            {dns.map((probe) => (
              <li key={probe.name} className="flex gap-3">
                <span className="w-28 shrink-0 text-fg-muted">{probe.name}</span>
                {probe.ms != null ? (
                  <span className="min-w-0 truncate tabular-nums">
                    {t("settings.dohOk", {
                      ms: Math.round(probe.ms),
                      address: probe.addresses.join(isZh ? "、" : ", "),
                    })}
                  </span>
                ) : (
                  <span className="text-danger">{probe.error}</span>
                )}
              </li>
            ))}
          </ul>
        )}
        <Row
          label={t("settings.netDiagLabel")}
          hint={t("settings.netDiagHint")}
          control={
            <div className="flex items-center gap-2">
              {probes && (
                <Button variant="ghost" onClick={() => void copyDiagnostics()}>
                  <Copy className="size-3.5" aria-hidden />
                  {t("settings.netDiagCopy")}
                </Button>
              )}
              <Button
                variant="secondary"
                disabled={probesBusy}
                onClick={() => void runDiagnostics()}
              >
                {probesBusy && <LightSpinner className="size-4" />}
                {probesBusy ? t("settings.netDiagRunning") : t("settings.netDiagRun")}
              </Button>
            </div>
          }
        />
        {probes && (
          <ul className="flex flex-col gap-1.5 px-4 py-3 text-xs">
            {probes.map((probe) => {
              const dotClass =
                probe.status === "ok"
                  ? "bg-accent"
                  : probe.status === "restricted"
                    ? "bg-warning"
                    : "bg-danger";
              const textClass =
                probe.status === "ok"
                  ? "text-fg"
                  : probe.status === "restricted"
                    ? "text-warning"
                    : "text-danger";
              return (
                <li key={probe.id} className="flex items-center gap-2.5">
                  <span className={`size-2 shrink-0 rounded-tag ${dotClass}`} aria-hidden />
                  <span className="w-32 shrink-0 font-medium text-fg">
                    {t(`settings.netDiagEndpoint.${probe.id}`)}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-fg-muted">
                    {probe.host}
                  </span>
                  <span className={`shrink-0 tabular-nums ${textClass}`}>
                    {t(`settings.netDiagStatus.${probe.status}`)}
                    {probe.latencyMs != null ? ` · ${Math.round(probe.latencyMs)} ms` : ""}
                    {probe.status !== "ok" && probe.httpStatus != null
                      ? ` (${probe.httpStatus})`
                      : ""}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </Group>
      <Group title={t("settings.claimGroup")}>
        <Row
          label={t("settings.claimFolder")}
          hint={t("settings.claimFolderHint")}
          control={
            <div className="flex min-w-0 items-center gap-2">
              <Truncated
                text={folder?.path ?? ""}
                className="max-w-52 select-text truncate text-fg-muted text-xs"
              >
                {folder?.custom ? folder.path : t("settings.claimFolderSystem")}
              </Truncated>
              {folder?.custom && (
                <Button variant="ghost" onClick={() => void resetFolder()}>
                  {t("settings.claimFolderReset")}
                </Button>
              )}
              <Button variant="secondary" onClick={() => void chooseFolder()}>
                {t("settings.claimFolderChange")}
              </Button>
            </div>
          }
        />
        <Row
          label={t("settings.moveClaimed")}
          hint={t("settings.moveClaimedHint")}
          control={
            <Switch
              label={t("settings.moveClaimed")}
              checked={download.moveClaimedFiles ?? false}
              onChange={(moveClaimedFiles) => set({ moveClaimedFiles })}
            />
          }
        />
      </Group>
    </>
  );
}
