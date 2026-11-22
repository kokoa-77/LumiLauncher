import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDown, ChevronUp, ExternalLink, Scale, ShieldCheck } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import authorKokoa77 from "@/assets/art/author-kokoa77.png";
import lumiMark from "@/assets/art/lumi-mark.png";
import type { AppInfo } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useAccountStore } from "@/stores/accounts";
import { SceneBackground } from "../art/SceneBackground";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Segmented } from "../ui/Segmented";
import { Tag } from "../ui/Tag";
import {
  APP_LICENSE,
  AUTHOR,
  type LicenseCategory,
  PROJECT_LINKS,
  SERVICE_CREDITS,
  THIRD_PARTY_ENTRIES,
} from "./aboutData";
import { Group, Row } from "./parts";
import { UpdateSettings } from "./UpdateSettings";

function openExternal(url: string, label: string) {
  openUrl(url).catch((error: unknown) => reportError(error, label));
}

/**
 * 设置「关于」页（发布前收口）：
 * 展示正式品牌标识、版本与独立第三方声明、自动更新、账号数据与隐私摘要、
 * BMCLAPI / MCIM / Modrinth / CurseForge 致谢，以及第三方开源许可弹窗（清单在 aboutData.ts）。
 */
export function AboutSettings({
  info,
  onCloseSettings,
}: {
  info: AppInfo;
  onCloseSettings: () => void;
}) {
  const { t, i18n } = useTranslation();
  const restartOnboarding = useAccountStore((s) => s.restartOnboarding);
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const [licenseOpen, setLicenseOpen] = useState(false);
  const [thirdPartyOpen, setThirdPartyOpen] = useState(false);
  const [category, setCategory] = useState<LicenseCategory>("rust");

  const isZh = i18n.language.toLowerCase().startsWith("zh");
  const counts = useMemo(
    () => ({
      rust: THIRD_PARTY_ENTRIES.filter((item) => item.category === "rust").length,
      frontend: THIRD_PARTY_ENTRIES.filter((item) => item.category === "frontend").length,
      reference: THIRD_PARTY_ENTRIES.filter((item) => item.category === "reference").length,
    }),
    [],
  );
  const visibleEntries = useMemo(
    () => THIRD_PARTY_ENTRIES.filter((item) => item.category === category),
    [category],
  );

  return (
    <>
      {/* 顶部微光矢量品牌横幅：叠加正式转正的原版柔光图标、标语与版本胶囊 */}
      <div className="relative h-28 overflow-hidden rounded-panel border border-line bg-linear-to-b from-sky-top to-sky-bottom">
        <SceneBackground kind="about" showInLite />
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0 bg-linear-to-t from-hero-scrim/85 via-hero-scrim/30 to-transparent"
        />
        <div className="relative flex h-full items-end justify-between gap-4 px-5 pb-4">
          <div className="flex min-w-0 items-center gap-3.5">
            <img
              src={lumiMark}
              alt=""
              width={48}
              height={48}
              className="size-12 shrink-0 select-none"
            />
            <div className="min-w-0">
              <p className="truncate font-semibold text-lg text-on-hero leading-tight">
                {t("app.name")}
              </p>
              <p className="mt-0.5 truncate text-on-hero-muted text-xs">
                {t("settings.aboutPage.tagline")}
              </p>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Tag tone="hero" className="font-mono tabular-nums">
              v{info.version}
            </Tag>
            <Tag tone="hero" className="font-mono">
              {APP_LICENSE.spdx}
            </Tag>
          </div>
        </div>
      </div>

      {/* 开发者署名卡片：点击直达 GitHub 主页 */}
      <Group>
        <button
          type="button"
          onClick={() => openExternal(AUTHOR.url, t("settings.aboutPage.developerOpen"))}
          className="group flex w-full items-center justify-between gap-4 px-4 py-3 text-left transition-colors duration-(--motion-press) hover:bg-surface-hover"
        >
          <div className="flex min-w-0 items-center gap-3.5">
            <img
              src={authorKokoa77}
              alt={AUTHOR.name}
              width={40}
              height={40}
              className="size-10 shrink-0 rounded-full border border-line object-cover select-none"
            />
            <div className="min-w-0">
              <p className="truncate font-semibold text-sm text-fg transition-colors duration-(--motion-press) group-hover:text-accent">
                {AUTHOR.name}
              </p>
              <p className="mt-0.5 truncate text-fg-muted text-xs">
                {t("settings.aboutPage.developerRole")}
              </p>
            </div>
          </div>
          <span className="inline-flex shrink-0 items-center gap-1.5 text-fg-muted text-xs transition-colors duration-(--motion-press) group-hover:text-accent">
            <span>{AUTHOR.handle}</span>
            <ExternalLink className="size-3.5" aria-hidden />
          </span>
        </button>
      </Group>

      {/* 基础信息、独立声明、仓库与许可证 */}
      <Group>
        <Row
          label={t("settings.independent")}
          hint={t("settings.independentHint")}
          control={null}
        />
        <Row
          label={t("settings.aboutPage.projectTitle")}
          control={
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                onClick={() => openExternal(PROJECT_LINKS.repo, "打开项目主页")}
              >
                <ExternalLink className="size-3.5" aria-hidden />
                {t("settings.aboutPage.repoAction")}
              </Button>
              <Button
                variant="secondary"
                onClick={() => openExternal(PROJECT_LINKS.releases, "打开发布页")}
              >
                {t("settings.aboutPage.releasesAction")}
              </Button>
            </div>
          }
        />
        <Row
          label={t("settings.aboutPage.licenseTitle")}
          hint={t("settings.aboutPage.licenseHint")}
          control={
            <Button variant="secondary" onClick={() => setLicenseOpen(true)}>
              <Scale className="size-3.5" aria-hidden />
              {t("settings.aboutPage.licenseAction")}
            </Button>
          }
        />
        {/* 窗口有没有拿到系统的云母效果：有人说「背景是灰的、不透明」时看这一行 */}
        <Row
          label={t("settings.backdrop", {
            backdrop: t(
              info.backdrop === "mica" ? "settings.backdropMica" : "settings.backdropSolid",
            ),
          })}
          control={null}
        />
      </Group>

      <UpdateSettings />

      {/* 账号数据与隐私说明（与 README / SECURITY.md / 完全一致） */}
      <Group title={t("settings.aboutPage.privacyGroup")}>
        <Row
          label={t("settings.aboutPage.privacyTitle")}
          hint={t("settings.aboutPage.privacyHint")}
          control={
            <Button variant="secondary" onClick={() => setPrivacyOpen((open) => !open)}>
              {privacyOpen ? (
                <ChevronUp className="size-3.5" aria-hidden />
              ) : (
                <ChevronDown className="size-3.5" aria-hidden />
              )}
              {t(privacyOpen ? "settings.aboutPage.collapse" : "settings.aboutPage.expand")}
            </Button>
          }
        />
        {privacyOpen && (
          <div className="flex flex-col gap-2.5 px-4 py-3 text-fg-muted text-xs leading-relaxed">
            <div className="flex items-start gap-2">
              <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-accent" aria-hidden />
              <p>{t("settings.aboutPage.privacyAuth")}</p>
            </div>
            <div className="flex items-start gap-2">
              <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-accent" aria-hidden />
              <p>{t("settings.aboutPage.privacyCredentials")}</p>
            </div>
            <div className="flex items-start gap-2">
              <ShieldCheck className="mt-0.5 size-3.5 shrink-0 text-accent" aria-hidden />
              <p>{t("settings.aboutPage.privacyRemoval")}</p>
            </div>
            <div className="pt-1">
              <Button
                variant="outline"
                size="sm"
                onClick={() => openExternal(PROJECT_LINKS.security, "打开安全说明")}
              >
                <ExternalLink className="size-3.5" aria-hidden />
                {t("settings.aboutPage.securityDoc")}
              </Button>
            </div>
          </div>
        )}
      </Group>

      {/* 外部服务与社区致谢（BMCLAPI / MCIM / Modrinth / CurseForge） */}
      <Group title={t("settings.aboutPage.creditsGroup")}>
        <div className="grid grid-cols-2 gap-2.5 p-3">
          {SERVICE_CREDITS.map((credit) => (
            <button
              key={credit.id}
              type="button"
              onClick={() => openExternal(credit.url, `打开 ${credit.name}`)}
              className="group flex flex-col gap-1 rounded-row border border-line bg-surface-2/60 p-3 text-left transition-colors duration-(--motion-press) hover:border-accent/45 hover:bg-surface-hover"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium text-fg text-sm">{credit.name}</span>
                <ExternalLink
                  className="size-3.5 shrink-0 text-fg-muted transition-colors duration-(--motion-press) group-hover:text-accent"
                  aria-hidden
                />
              </div>
              <p className="text-fg-muted text-xs leading-relaxed">
                {t(`settings.aboutPage.credit_${credit.id}`)}
              </p>
            </button>
          ))}
        </div>
        <p className="px-4 py-2.5 text-fg-muted text-xs">
          {t("settings.aboutPage.creditsFootnote")}
        </p>
      </Group>

      {/* 第三方开源许可清单入口 */}
      <Group title={t("settings.aboutPage.thirdPartyGroup")}>
        <Row
          label={t("settings.aboutPage.thirdPartyTitle")}
          hint={t("settings.aboutPage.thirdPartyHint", { count: THIRD_PARTY_ENTRIES.length })}
          control={
            <Button variant="secondary" onClick={() => setThirdPartyOpen(true)}>
              {t("settings.aboutPage.thirdPartyAction")}
            </Button>
          }
        />
      </Group>

      {/* 首次运行引导再走一遍：账号、实例、存档都不动 */}
      <Group>
        <Row
          label={t("settings.restartOnboarding")}
          hint={t("settings.restartOnboardingHint")}
          control={
            <Button
              onClick={() => {
                onCloseSettings();
                void restartOnboarding();
              }}
            >
              {t("settings.restartOnboardingAction")}
            </Button>
          }
        />
      </Group>

      {/* 本项目开源许可证弹窗 */}
      <Dialog
        open={licenseOpen}
        onClose={() => setLicenseOpen(false)}
        title={t("settings.aboutPage.licenseDialogTitle")}
        wide
        footer={
          <>
            <Button
              variant="outline"
              onClick={() => openExternal(PROJECT_LINKS.license, "查看完整许可证")}
            >
              <ExternalLink className="size-3.5" aria-hidden />
              {t("settings.aboutPage.licenseOpenFull")}
            </Button>
            <Button variant="secondary" onClick={() => setLicenseOpen(false)}>
              {t("dialog.close")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <Tag tone="accent" className="font-mono">
              {APP_LICENSE.spdx}
            </Tag>
            <span className="font-mono text-fg-muted text-xs">{APP_LICENSE.copyright}</span>
          </div>
          <p className="text-fg-muted text-xs leading-relaxed">
            {t("settings.aboutPage.licenseSummary")}
          </p>
          <div className="rounded-panel border border-line bg-surface-2/60 p-3.5">
            <p className="font-medium text-xs">{t("settings.aboutPage.licenseBrandTitle")}</p>
            <p className="mt-1 text-fg-muted text-xs leading-relaxed">
              {t("settings.aboutPage.licenseBrandBody")}
            </p>
          </div>
          <p className="text-fg-muted text-xs leading-relaxed">{t("settings.independentHint")}</p>
        </div>
      </Dialog>

      {/* 第三方开源许可详情弹窗 */}
      <Dialog
        open={thirdPartyOpen}
        onClose={() => setThirdPartyOpen(false)}
        title={t("settings.aboutPage.thirdPartyDialogTitle")}
        wide
        footer={
          <Button variant="secondary" onClick={() => setThirdPartyOpen(false)}>
            {t("dialog.close")}
          </Button>
        }
      >
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Segmented<LicenseCategory>
              label={t("settings.aboutPage.thirdPartyDialogTitle")}
              value={category}
              options={[
                {
                  value: "rust",
                  label: t("settings.aboutPage.categoryRust", { count: counts.rust }),
                },
                {
                  value: "frontend",
                  label: t("settings.aboutPage.categoryFrontend", { count: counts.frontend }),
                },
                {
                  value: "reference",
                  label: t("settings.aboutPage.categoryReference", { count: counts.reference }),
                },
              ]}
              onChange={setCategory}
            />
          </div>
          <div className="max-h-80 divide-y divide-line overflow-y-auto rounded-panel border border-line bg-surface">
            {visibleEntries.map((entry) => (
              <div
                key={`${entry.category}:${entry.name}`}
                className="flex items-start justify-between gap-4 px-3.5 py-2.5"
              >
                <div className="min-w-0 flex-1">
                  <p className="font-medium font-mono text-fg text-xs">{entry.name}</p>
                  <p className="mt-0.5 text-fg-muted text-xs leading-relaxed">
                    {isZh ? entry.usage.zh : entry.usage.en}
                  </p>
                </div>
                <Tag className="mt-0.5 font-mono">{entry.license}</Tag>
              </div>
            ))}
          </div>
          <p className="text-fg-muted text-xs">{t("settings.aboutPage.thirdPartyFootnote")}</p>
        </div>
      </Dialog>
    </>
  );
}
