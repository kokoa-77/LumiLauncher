import {
  Boxes,
  Check,
  Compass,
  FolderOpen,
  FolderPlus,
  House,
  type LucideIcon,
  Play,
  Sparkles,
} from "lucide-react";
import { type Dispatch, type ReactNode, type SetStateAction, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { unwrap } from "@/lib/api";
import {
  commands,
  type EffectsMode,
  type FolderFound,
  type GameDirInfo,
  type InstanceSummary,
  type RecentWorld,
  type Settings,
  type ThemeMode,
} from "@/lib/bindings";
import { resolveEffects } from "@/lib/effects";
import { logError, reportError } from "@/lib/report";
import { useAccountStore, useCurrentAccount, useSkin } from "@/stores/accounts";
import { useAppStore } from "@/stores/app";
import { useGameStore } from "@/stores/game";
import { AccountForm } from "../account/AccountForm";
import { SkinControls } from "../account/SkinPanel";
import { PlayerHead } from "../PlayerHead";
import { AccentPicker } from "../settings/AccentPicker";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Segmented } from "../ui/Segmented";
import { Tag } from "../ui/Tag";
import { type FindSelection, findKey, runFindSelection } from "./findGames";
import { MiniHome } from "./MiniHome";
import { Caption, StepFrame } from "./parts";
import type { Found } from "./useFound";

/** 一行能勾的东西（整行都能点）。`done`：已经做过了（已挂上），勾着、点不了，但不变灰 */
function PickRow({
  checked,
  disabled = false,
  done = false,
  label,
  onChange,
  children,
  aside,
}: {
  checked: boolean;
  disabled?: boolean;
  done?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
  children: ReactNode;
  aside: ReactNode;
}) {
  const locked = disabled || done;
  return (
    <li>
      <label
        className={`flex items-center gap-3 rounded-panel border border-line bg-surface px-4 py-3 shadow-raised transition-colors duration-(--motion-press) ${
          disabled ? "opacity-60" : done ? "" : "cursor-pointer hover:bg-surface-hover"
        }`}
      >
        <Checkbox
          checked={checked}
          disabled={locked}
          label={label}
          onChange={(value) => onChange(value)}
        />
        <div className="min-w-0 flex-1">{children}</div>
        <div className="flex shrink-0 items-center gap-1.5">{aside}</div>
      </label>
    </li>
  );
}

function toggled(set: Set<string>, id: string, on: boolean): Set<string> {
  const next = new Set(set);
  if (on) next.add(id);
  else next.delete(id);
  return next;
}

/**
 * 导入已有游戏。已经挂上的列出来（打勾、标「已挂上」）；找到还没挂上的 .minecraft（官方、PCL、HMCL）默认勾上，
 * 直接挂上、不复制；别家的实例（Prism、Modrinth App……）要复制一份、补文件，默认不勾，勾上的在后台导入；
 * 找不到的「添加文件夹」自己选（.minecraft 和别家实例都认）
 */
export function FindStep({
  found,
  addFolder,
  selection,
  setSelection,
  onNext,
  onBack,
}: {
  found: Found;
  addFolder: (folder: FolderFound) => { roots: string[]; foreign: string[] };
  selection: FindSelection;
  setSelection: Dispatch<SetStateAction<FindSelection>>;
  onNext: () => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const refreshInstances = useGameStore((s) => s.refreshInstances);
  const refreshRecentWorlds = useGameStore((s) => s.refreshRecentWorlds);
  const [info, setInfo] = useState<GameDirInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const needsRetry = selection.retry.length > 0;
  const editingLocked = busy || needsRetry;
  const results = Object.values(selection.outcomes);
  const succeeded = results.filter((item) => item.error === null).length;
  const failed = results.length - succeeded;

  useEffect(() => {
    commands
      .gameDirInfo()
      .then(setInfo)
      .catch((error: unknown) => logError(error, "读取游戏目录"));
  }, []);

  const choose = async () => {
    try {
      const chosen = unwrap(await commands.gameDirChoose());
      if (chosen) {
        setInfo(chosen);
        await Promise.all([refreshInstances(), refreshRecentWorlds()]);
      }
    } catch (error) {
      reportError(error, "换游戏目录");
    }
  };

  const pickFolder = async () => {
    try {
      const folder = unwrap(await commands.linkedRootsPick());
      if (!folder) return;
      const added = addFolder(folder);
      if (folder.kind === "gameRoot" && added.roots.length === 0) {
        toast(t("linked.already", { name: folder.root.name }));
      }
      if (added.foreign.length > 0) {
        setSelection((prev) => ({
          ...prev,
          imports: new Set([...prev.imports, ...added.foreign]),
        }));
      }
    } catch (error) {
      reportError(error, "添加文件夹");
    }
  };

  const proceed = async () => {
    setBusy(true);
    useAccountStore.setState({ onboardingFindBlocked: true });
    let next = selection;
    try {
      next = await runFindSelection(found, selection);
      setSelection(next);
    } finally {
      useAccountStore.setState({ onboardingFindBlocked: next.retry.length > 0 });
      setBusy(false);
    }
    if (next.retry.length === 0) onNext();
  };

  const continueAnyway = () => {
    setSelection((prev) => ({ ...prev, retry: [] }));
    useAccountStore.setState({ onboardingFindBlocked: false });
    onNext();
  };

  const roots = found.linked.length + found.roots.length;
  const nothing = !found.loading && roots === 0 && found.foreign.length === 0;
  return (
    <StepFrame
      title={t("onboarding.find.title")}
      hint={t("onboarding.find.hint")}
      footer={
        <div className="flex w-full flex-wrap items-center gap-3">
          <Button variant="ghost" disabled={editingLocked} onClick={onBack}>
            {t("onboarding.back")}
          </Button>
          <p role="status" className="mr-auto text-fg-muted text-sm">
            {results.length > 0 ? t("onboarding.find.result", { succeeded, failed }) : null}
          </p>
          {needsRetry && (
            <Button variant="ghost" disabled={busy} onClick={continueAnyway}>
              {t("onboarding.find.continueAnyway")}
            </Button>
          )}
          {found.loading ? (
            <Button variant="primary" onClick={onNext}>
              {t("onboarding.find.skipSearch")}
            </Button>
          ) : (
            <Button variant="primary" disabled={busy} onClick={() => void proceed()}>
              {t(
                busy
                  ? "onboarding.find.working"
                  : needsRetry
                    ? "onboarding.find.retry"
                    : "onboarding.next",
              )}
            </Button>
          )}
        </div>
      }
    >
      <div className="flex flex-col gap-6">
        {found.loading && <p className="text-fg-muted text-sm">{t("onboarding.find.searching")}</p>}
        {nothing && <p className="text-fg-muted text-sm">{t("onboarding.find.nothing")}</p>}
        {(roots > 0 || found.foreign.length > 0) && (
          <h2 className="-mb-3 font-medium text-sm">{t("onboarding.find.foundHere")}</h2>
        )}
        {roots > 0 && (
          <section>
            <Caption>{t("onboarding.find.linkCaption")}</Caption>
            <ul className="flex flex-col gap-2">
              {found.linked.map((root) => (
                <PickRow
                  key={root.id}
                  checked
                  done
                  label={t("onboarding.find.link", { name: root.name })}
                  onChange={() => undefined}
                  aside={
                    <>
                      <Tag tone="accent">
                        <Check className="mr-1 size-3" aria-hidden />
                        {t("onboarding.find.linked")}
                      </Tag>
                      <Tag>{t("linked.versions", { count: root.versions })}</Tag>
                    </>
                  }
                >
                  <p className="truncate font-medium text-sm">{root.name}</p>
                  <p className="wrap-anywhere text-fg-muted text-xs">{root.path}</p>
                </PickRow>
              ))}
              {found.roots.map((root) => {
                const outcome = selection.outcomes[findKey("link", root.id)];
                const done = outcome?.error === null;
                return (
                  <PickRow
                    key={root.id}
                    checked={!selection.excludedLinks.has(root.id)}
                    disabled={editingLocked && !done}
                    done={done}
                    label={t("onboarding.find.link", { name: root.name })}
                    onChange={(on) =>
                      setSelection((prev) => ({
                        ...prev,
                        excludedLinks: toggled(prev.excludedLinks, root.id, !on),
                      }))
                    }
                    aside={
                      <>
                        {done ? (
                          <Tag tone="accent">
                            <Check className="mr-1 size-3" aria-hidden />
                            {t("onboarding.find.linked")}
                          </Tag>
                        ) : (
                          <Tag>{t(`launchers.${root.launcher}`)}</Tag>
                        )}
                        <Tag>{t("linked.versions", { count: root.versions })}</Tag>
                      </>
                    }
                  >
                    <p className="truncate font-medium text-sm">{root.name}</p>
                    <p className="wrap-anywhere text-fg-muted text-xs">{root.path}</p>
                    {outcome?.error && <p className="text-danger text-xs">{outcome.error}</p>}
                  </PickRow>
                );
              })}
            </ul>
          </section>
        )}
        {found.foreign.length > 0 && (
          <section>
            <Caption>{t("onboarding.find.importCaption")}</Caption>
            <ul className="flex flex-col gap-2">
              {found.foreign.map((item) => {
                const outcome = selection.outcomes[findKey("import", item.id)];
                const done = outcome?.error === null;
                return (
                  <PickRow
                    key={item.id}
                    checked={selection.imports.has(item.id)}
                    disabled={!done && (!item.supported || editingLocked)}
                    done={done}
                    label={t("onboarding.find.import", { name: item.name })}
                    onChange={(on) =>
                      setSelection((prev) => ({
                        ...prev,
                        imports: toggled(prev.imports, item.id, on),
                      }))
                    }
                    aside={
                      done ? (
                        <Tag tone="accent">
                          <Check className="mr-1 size-3" aria-hidden />
                          {t("onboarding.find.imported")}
                        </Tag>
                      ) : (
                        <Tag>
                          {item.supported
                            ? t(`launchers.${item.launcher}`)
                            : t("onboarding.find.unsupported")}
                        </Tag>
                      )
                    }
                  >
                    <p className="truncate font-medium text-sm">{item.name}</p>
                    <p className="truncate text-fg-muted text-xs">
                      {[item.gameVersion, item.loader].filter(Boolean).join(" · ")}
                    </p>
                    {outcome?.error && <p className="text-danger text-xs">{outcome.error}</p>}
                  </PickRow>
                );
              })}
            </ul>
          </section>
        )}
        {!found.loading && (
          <div className="flex items-center justify-between gap-3">
            <p className="text-fg-muted text-xs">{t("onboarding.find.pickHint")}</p>
            <Button size="sm" disabled={editingLocked} onClick={() => void pickFolder()}>
              <FolderPlus className="size-3.5" aria-hidden />
              {t("onboarding.find.pick")}
            </Button>
          </div>
        )}
        <section>
          <Caption>{t("onboarding.find.own")}</Caption>
          <div className="flex items-center gap-3 rounded-panel border border-line bg-surface px-4 py-3 shadow-raised">
            <span className="grid size-9 shrink-0 place-items-center rounded-row bg-accent-soft text-accent">
              <FolderOpen className="size-4" aria-hidden />
            </span>
            <p className="min-w-0 flex-1 wrap-anywhere text-sm">
              {info?.path ?? t("common.loading")}
            </p>
            <Button
              size="sm"
              disabled={!info || info.locked || editingLocked}
              onClick={() => void choose()}
            >
              {t("onboarding.find.change")}
            </Button>
          </div>
        </section>
      </div>
    </StepFrame>
  );
}

/** 第 2 步：登录 Microsoft 或跳过，稍后再登录。 */
export function AccountStep({ onNext, onBack }: { onNext: () => void; onBack: () => void }) {
  const { t } = useTranslation();
  const current = useCurrentAccount();
  const skin = useSkin(current);
  const [adding, setAdding] = useState(current === null);
  const showForm = adding || current === null;
  return (
    <StepFrame
      title={showForm ? t("onboarding.account.title") : t("onboarding.account.titleDone")}
      hint={showForm ? t("onboarding.account.hint") : undefined}
      footer={
        <>
          <Button variant="ghost" onClick={onBack}>
            {t("onboarding.back")}
          </Button>
          {current ? (
            <Button variant="primary" onClick={onNext}>
              {t("onboarding.next")}
            </Button>
          ) : (
            <Button onClick={onNext}>{t("onboarding.skip")}</Button>
          )}
        </>
      }
    >
      {showForm ? (
        <div className="flex flex-col gap-3">
          <AccountForm onDone={() => setAdding(false)} />
          {current && (
            <button
              type="button"
              className="self-start text-accent text-xs hover:underline"
              onClick={() => setAdding(false)}
            >
              {t("onboarding.account.keep", { name: current.name })}
            </button>
          )}
        </div>
      ) : (
        current && (
          <div className="flex flex-col gap-6">
            <div className="flex items-center gap-4 rounded-panel border border-line bg-surface p-4 shadow-raised">
              <PlayerHead name={current.name} skin={skin} className="size-10" />
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium">{current.name}</p>
                <Tag>{t("account.microsoft.label")}</Tag>
              </div>
              <Button size="sm" onClick={() => setAdding(true)}>
                {t("onboarding.account.another")}
              </Button>
            </div>
            <SkinControls />
          </div>
        )
      )}
    </StepFrame>
  );
}

const THEMES: ThemeMode[] = ["system", "light", "dark"];

/** 选深浅色的小卡片：里面是那一种深浅色的迷你首页（跟着系统的是斜着一半一半） */
function ThemeTile({
  mode,
  selected,
  label,
  onPick,
}: {
  mode: ThemeMode;
  selected: boolean;
  label: string;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onPick}
      className={`flex flex-col gap-2 rounded-panel border p-2 text-left transition-colors duration-(--motion-press) ${
        selected ? "border-accent bg-accent-soft" : "border-line bg-surface hover:bg-surface-hover"
      }`}
    >
      <div className="relative">
        <div data-theme={mode === "dark" ? "dark" : "light"}>
          <MiniHome small />
        </div>
        {mode === "system" && (
          <div
            data-theme="dark"
            className="absolute inset-0 [clip-path:polygon(62%_0,100%_0,100%_100%,38%_100%)]"
          >
            <MiniHome small />
          </div>
        )}
      </div>
      <span className={`px-1 text-sm ${selected ? "font-medium text-accent" : ""}`}>{label}</span>
    </button>
  );
}

/** 第 3 步：外观。深浅色、主色、画面效果；左边的迷你首页实时跟着变 */
export function LookStep({ onNext, onBack }: { onNext: () => void; onBack: () => void }) {
  const { t } = useTranslation();
  const settings = useAppStore((s) => s.settings);
  const saveSettings = useAppStore((s) => s.saveSettings);
  const lowEnd = useAppStore((s) => s.lowEnd);
  const update = (change: (current: Settings) => Settings) => {
    if (settings) void saveSettings(change(settings));
  };
  const detected = resolveEffects("auto", lowEnd ?? false);
  return (
    <StepFrame
      title={t("onboarding.look.title")}
      footer={
        <>
          <Button variant="ghost" onClick={onBack}>
            {t("onboarding.back")}
          </Button>
          <Button variant="primary" onClick={onNext}>
            {t("onboarding.next")}
          </Button>
        </>
      }
    >
      {settings && (
        <div className="flex flex-col gap-6">
          <section>
            <Caption>{t("onboarding.look.theme")}</Caption>
            <div
              className="grid grid-cols-3 gap-3"
              role="radiogroup"
              aria-label={t("onboarding.look.theme")}
            >
              {THEMES.map((mode) => (
                <ThemeTile
                  key={mode}
                  mode={mode}
                  selected={settings.appearance.theme === mode}
                  label={t(`onboarding.look.themes.${mode}`)}
                  onPick={() =>
                    update((s) => ({ ...s, appearance: { ...s.appearance, theme: mode } }))
                  }
                />
              ))}
            </div>
          </section>
          <section>
            <Caption>{t("accent.label")}</Caption>
            <AccentPicker settings={settings} update={update} />
          </section>
          <section>
            <Caption>{t("settings.effects")}</Caption>
            <div className="flex flex-wrap items-center gap-3">
              <Segmented<EffectsMode>
                label={t("settings.effects")}
                value={settings.appearance.effects}
                options={[
                  { value: "auto", label: t("settings.effects_auto") },
                  { value: "rich", label: t("settings.effects_rich") },
                  { value: "lite", label: t("settings.effects_lite") },
                ]}
                onChange={(effects) =>
                  update((s) => ({ ...s, appearance: { ...s.appearance, effects } }))
                }
              />
              <span className="text-fg-muted text-xs">
                {t("onboarding.look.effectsHint", { level: t(`settings.effects_${detected}`) })}
              </span>
            </div>
          </section>
        </div>
      )}
    </StepFrame>
  );
}

/** 准备好了之后做什么（欢迎页「开始使用」时照着做） */
export type ReadyChoice = "continue" | "new" | "discover" | "library" | "later";

/** 一个能选的去处：选中是主色浅底 + 主色字（开发合同「选中」） */
function ChoiceCard({
  icon: Icon,
  title,
  hint,
  selected,
  onSelect,
}: {
  icon: LucideIcon;
  title: string;
  hint: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={`flex items-center gap-4 rounded-panel border p-4 text-left transition-colors duration-(--motion-press) ${
        selected
          ? "border-accent bg-accent-soft"
          : "border-line bg-surface shadow-raised hover:bg-surface-hover"
      }`}
    >
      <span
        className={`grid size-10 shrink-0 place-items-center rounded-row ${
          selected ? "bg-accent text-on-accent" : "bg-accent-soft text-accent"
        }`}
      >
        <Icon className="size-5" aria-hidden />
      </span>
      <span className="min-w-0 flex-1">
        <span className={`block font-medium text-sm ${selected ? "text-accent" : ""}`}>
          {title}
        </span>
        <span className="block text-fg-muted text-xs">{hint}</span>
      </span>
      {selected && <Check className="size-4 shrink-0 text-accent" aria-hidden />}
    </button>
  );
}

/**
 * 准备好了。默认「先随便看看」，有可进入的世界时也提供继续游戏。
 * 右下角「下一步」进整窗的欢迎页，在那里点「开始使用」才真的去做
 */
export function ReadyStep({
  ready,
  choice,
  onChoose,
  onNext,
  onBack,
}: {
  ready: { world: RecentWorld; instance: InstanceSummary | undefined } | null | undefined;
  choice: ReadyChoice;
  onChoose: (choice: ReadyChoice) => void;
  onNext: () => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const instances = useGameStore((s) => s.instances);
  const importing = useGameStore(
    (s) =>
      Object.values(s.jobs).filter(
        (j) => j.key.startsWith("instance-create:import:") && j.state === "running",
      ).length,
  );
  const recent = ready?.world;
  const world = recent?.world;
  const card = (key: ReadyChoice, icon: LucideIcon, title: string, hint: string) => (
    <ChoiceCard
      key={key}
      icon={icon}
      title={title}
      hint={hint}
      selected={choice === key}
      onSelect={() => onChoose(key)}
    />
  );
  return (
    <StepFrame
      title={t("onboarding.ready.title")}
      hint={importing > 0 ? t("onboarding.ready.importing", { count: importing }) : undefined}
      footer={
        <>
          <Button variant="ghost" onClick={onBack}>
            {t("onboarding.back")}
          </Button>
          <Button variant="primary" disabled={ready === undefined} onClick={onNext}>
            {t("onboarding.next")}
          </Button>
        </>
      }
    >
      {ready === undefined ? (
        <p className="text-fg-muted text-sm">{t("common.loading")}</p>
      ) : (
        <div
          className="flex flex-col gap-2"
          role="radiogroup"
          aria-label={t("onboarding.ready.title")}
        >
          <Caption>{t("onboarding.ready.what")}</Caption>
          {card("later", House, t("onboarding.play.later"), t("onboarding.play.laterHint"))}
          {card("new", Sparkles, t("onboarding.play.new"), t("onboarding.play.newHint"))}
          {card(
            "discover",
            Compass,
            t("onboarding.play.discover"),
            t("onboarding.play.discoverHint"),
          )}
          {recent &&
            world &&
            card(
              "continue",
              Play,
              recent.quickPlay
                ? t("onboarding.ready.continueTitle", { name: world.name })
                : t("onboarding.ready.startGame"),
              [
                ready?.instance?.name,
                !recent.quickPlay ? t("onboarding.ready.singleplayer", { name: world.name }) : null,
                world.day !== null ? t("home.world.day", { n: world.day }) : null,
              ]
                .filter(Boolean)
                .join(" · "),
            )}
          {!world &&
            instances.length > 0 &&
            card(
              "library",
              Boxes,
              t("onboarding.play.ready", { count: instances.length }),
              t("onboarding.play.readyHint"),
            )}
        </div>
      )}
    </StepFrame>
  );
}
