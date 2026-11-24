# Lumi Design & Engineering Rules

These are the rules every change to Lumi must follow: layering, the IPC contract, design tokens, motion, security, error handling, required checks and performance budgets. Pull requests that break them will not be merged.

This public version mirrors the maintainer's internal development contract, with the same section numbers — when a code comment says "section 5" or "section 8, rule 6", it refers to this document. If a passing idea conflicts with this document, this document wins.

See [ARCHITECTURE.md](../ARCHITECTURE.md) for the code layout and [CONTRIBUTING.md](../CONTRIBUTING.md) for how to contribute.

## 1. Product Statement & Decision Rule

**A launcher where there is always a game ready to click** — the fewest possible clicks from double-clicking the icon to being in the game.

Every new feature starts with one question: **does it get players into the game faster, or slower?** If slower, it is not built, or it waits.

## 2. Layers & Dependency Direction

```
app/src (React UI)
   │  calls the backend only through the generated functions in app/src/lib/bindings.ts
   ▼
app/src-tauri (app shell) ── commands / events / windows / logging
   │  calls crates/core only; no business logic here
   ▼
crates/core (core library, pure Rust) ── also used by crates/cli (command-line debugging tool)
```

| From | To | Allowed |
|---|---|---|
| UI | generated `commands` / `events` from `@/lib/bindings` | ✅ |
| UI | raw `invoke("string")` calls | ❌ |
| App shell | `crates/core` | ✅ |
| `crates/core` | `tauri` or any UI-related crate | ❌ **Never** |

Inside the core library, from bottom to top (lower layers must not depend on higher ones):

```
fs / config / redact / jobs  ←  net  ←  meta  ←  java / auth  ←  install / content / modpack  ←  launch
```

## 3. UI Boundaries

- Do not add plugins that touch the system directly, such as `@tauri-apps/plugin-fs` or `@tauri-apps/plugin-shell`
- The UI never builds file paths and never touches tokens. It only knows **IDs** (instance ID, account ID, job ID); the core library resolves paths
- External links open only through `@tauri-apps/plugin-opener`, handed to the system browser

## 4. Interface Contract

- Command names follow `<domain>_<action>`, e.g. `settings_get`, `jobs_cancel` (they become `commands.settingsGet` in the frontend)
- Commands that can fail return `Result<T, AppError>`. `AppError` has three fields: `code` (`<domain>.<reason>`), `message` (a plain-language Chinese message) and `detail` (redacted technical details). The English UI looks up its translation by `code` under `error.backend.*` in `en.json`
- The frontend unwraps results with `unwrap()` from `@/lib/api`; failures throw `CommandError`
- When adding or changing a command, change the core function, `commands.rs`, the registration in `lib.rs` and the regenerated `bindings.ts` **in the same commit**
- `bindings.ts` is generated — **never edit it by hand**; run `cargo test -p launcher-app` to regenerate it
- Numbers crossing the process boundary are always `f64` / `u32`, never `u64` / `i64` (JavaScript loses precision)

## 5. Copy & Tokens

### Copy

- All user-visible text goes through i18n (`app/src/locales/zh-CN.json` and `en.json`; both files must have identical keys and placeholders, enforced by a unit test). Keys follow `<page>.<section>.<name>`. Logs and code comments are exempt, as is the dev-only component gallery (`app/src/dev/`)
- **Hint text** (the grey line under a setting, the intro of a dialog, input placeholders) **follows the existing copy**: written, concise, one or two sentences saying what the option controls and when to change it or what to do if something goes wrong (e.g. "Number of files downloaded at the same time. Lower this if your connection drops."). Avoid casual phrasing. New hints follow the tone of the existing ones
- Hidden content must be visible on hover: "+N" uses `MoreTag` from `ui/HoverCard` to list the hidden items, names or descriptions that may be truncated use `Truncated`, and details that are hidden by design use `HoverHint`. Never use the browser's native `title` tooltip

### Tokens

- **Colors, radii, shadows, fonts and motion** come only from the tokens in `app/src/styles/tokens.css` (through Tailwind semantic classes: `bg-surface`, `text-fg-muted`, `border-line`, `bg-accent`…)
- No color literals in components (`#fff`, `rgb()`, `oklch()`); Tailwind's built-in palette is disabled
- Switching light / dark or the Mica backdrop only changes variables, never components
- Layers floating above other content (dropdowns, dialogs, panels, capsules, drawers) always use `bg-overlay` (always opaque), never `bg-surface` (semi-transparent under Mica, so text shows through)
- The accent color is given only as "hue + chroma"; lightness is computed by `lib/color.ts` from contrast (text on the accent and accent-colored text must both reach ≥ 4.5). Changing the accent goes through `lib/themeEngine.ts`
- No live blur (`backdrop-blur`, `filter: blur`): it is the heaviest effect on old computers. Behind overlays, only dim; text on screenshots relies on gradient scrims

### Corner Radii

Use only the named levels: `rounded-row` (list rows, icons), `rounded-control` (buttons, inputs, selects, segmented controls), `rounded-panel` (panels, cards, menus), `rounded-dialog` (dialogs), `rounded-cta` (the one primary button on a page), `rounded-icon` (icon buttons, avatars), `rounded-tag` (tags). Two rules:

1. **Every clickable control is a pill**, and controls in the same row share the same height
2. **Containers are rounded rectangles, and outer containers are rounder than inner ones**. Small icon tiles and callouts are not controls and must not use `rounded-control`

### The Six Color Roles

The same everywhere — never one style here and another there:

| Role | Look | Used for |
|---|---|---|
| Selected (where you are) | Soft accent background `bg-accent-soft` + accent text `text-accent` | Sidebar, tabs, segmented controls, the selected item in lists and dropdowns, the settings dialog sidebar, the current page — no exceptions |
| Hovered (mouse or keyboard focus) | Grey background `bg-surface-hover` | Never the soft accent background; in the command palette, the focused row gets an accent bar on its left |
| The single most important button on a page | Solid accent `bg-accent` (`CtaButton` or `Button` primary) | Continue, Play, New instance |
| Repeated actions | Accent outline + accent text, soft fill only on hover | The install button on every card; the play button on instance cards uses the `card` variant of `IconButton` |
| Done (good news) | Soft accent background + accent text + check mark (favorited is a solid accent ♥) | Installed, favorited, compatible with this instance; never grey, never red |
| Information only | Grey with a thin border | All tags |
| Danger | Red | Delete, stop the game |

### Depth

- Depth comes from background layers, not heavy shadows: window background `bg-bg` → content panel `bg-surface-2` → cards and controls `bg-surface` (with the very light `shadow-raised`). The dark theme uses a fixed night-sky blue (`--night-h`) that does not follow the world's colors
- Text and small capsules on top of world screenshots use the `on-hero` and `hero-chip` tokens, and the gradient scrim uses `hero-scrim`; never write your own white-on-black
- Under Mica, the window background carries a very faint sky tint so the sidebar and title bar never show the system's neutral grey

### Same Kind of Thing, Same Place, Same Look

- **Page container**: every page's outermost element uses `PAGE` / `PAGE_X` from `lib/layout.ts` (max width 1152, 32 px side padding); pages never set their own `max-w-*`
- **Going back**: only the group at the top left of the title bar — "Lumi" (home), back / forward (mouse side buttons and Alt + ← / → also work), and the breadcrumb (click an earlier level to jump back). Pages never contain "← Destination" back links
- **Clickable things** show a hand cursor on hover (handled once in `global.css`); disabled ones keep the arrow. A clickable `div` must add `cursor-pointer` itself
- **List pages**: the title row holds the page's most important button; the row below holds search and filters; the view switch is called "Cards / List" and sits at the far right of the row directly above the list
- **Tabs** (`ui/Tabs`) switch "which part of this page you are looking at"; **segmented controls** (`ui/Segmented`) switch "filters". Never mix them. Actions on the right of a tab toolbar are always outlined secondary buttons
- **Font sizes** are limited to: 11 (`text-2xs`, badges), 12 (captions), 14 (body), 16, 18 (dialog titles), 20 (home screen primary button), 24 (page titles), 30 (banner titles), 56 (`text-hero`, world name on the home screen). Never write `text-[13px]`
- **Section titles** use `ui/SectionTitle`: 14 px medium weight + a grey count; group labels inside panels are 12 px grey
- **Dialogs** all have a "×" at the top right (built into `ui/Dialog`); Esc and clicking the backdrop still close them; with stacked dialogs, Esc closes only the top one
- **Window buttons** (minimize, maximize, close): the hit area is a full title-bar-height square; the visible hover is a circle, and close hovers as a red circle
- **All settings are dialogs sharing one shell** (`components/settings/SettingsShell`): global settings (title bar gear / Ctrl + ,) and instance settings (the gear left of "Play" in the instance banner). Groups an instance can override have no toggle: the controls are always shown, displaying the current global value until changed, with a grey "Following global" label with a globe next to the title. Changing anything makes the group the instance's own, and the label turns into an accent "✏ This instance ×"; clicking × returns it to following global

## 6. Motion

- Only three easing curves: `ease-out` (enter), `ease-in` (exit), `ease-in-out` (move). Only two Motion springs: `spring.gentle` and `spring.snappy` (`app/src/lib/motion.ts`)
- Only five durations: 100 (hover / press), 180 (toggle / switch), 220 (popovers), 260 (elements entering and leaving), 340 (pages) ms, **capped at 400 ms**
- Animate only `transform` and `opacity`; `transition: all` is forbidden; never animate width, height or position
- Exception one: the accent color ("accent follows the instance"). The five `--accent-*` variables transition together over 340 ms in `tokens.css`, only in "Rich" mode without reduced motion. Components never write their own color transitions; changing the color only calls `applyAccent` in `lib/themeEngine.ts`
- Exception two: ambient light effects (background motes, breathing glow, the very slow push-in of the home screen cover, the progress bar shimmer). Their period may exceed 400 ms, but they animate only `transform` and `opacity`; at most one breathing glow per screen and no more than 24 motes (at most 8 at the bottom of the home screen when it has a world cover); colors only from tokens such as `--mote` and `--accent`; **they must stop in Lightweight mode and under reduced motion**. Use them only on "scene" screens such as onboarding, the installer and the home screen, never on working screens such as lists and settings
- Interface sounds: synthesized with Web Audio only, no third-party dependencies, no network, off by default, and activated only after user interaction
- Always respect reduced motion (both the system setting and the app setting) and "Visual effects: Lightweight" (`data-fx="lite"`, enabled automatically on old computers): transitions get shorter, movement and scaling animations turn into plain fades. Every new large animation or layer must define how it degrades in Lightweight mode

## 7. Long Lists

Lists that may exceed 30 items must be virtualized (`@tanstack/react-virtual`, see `components/VirtualList.tsx`). No heavy shadows inside list items.

## 8. Security (Violations Will Not Be Merged)

1. The UI never touches paths or secrets (see section 3)
2. Every untrusted path (file names inside modpacks, archive entries, file names returned by the network) **must go through `launcher_core::fs::safe_join`**
3. Every configuration write uses `launcher_core::fs::write_json_atomic` (atomic write)
4. Every download is verified by hash and size; on mismatch it is deleted and downloaded again
5. All logs are redacted (file logs are redacted automatically; wrap token-like values in `launcher_core::redact::Secret`)
6. When launching Java or running installer processors, pass arguments as an array — **never join them into a single command string**
7. Metadata and files come directly from official sources (Mojang, each mod loader, Modrinth, CurseForge). Never use third-party game-file mirrors or other launchers' own metadata servers. CurseForge downloads use only the URLs the platform provides; files whose authors disallow third-party downloads go through the manual download flow — **never derive or guess CDN URLs**. The official API uses Lumi's own key, and **the key is sent only to CurseForge's own domains**
8. Never copy code from any GPL project or from PCL. When porting MIT / Apache code, state its source and license at the top of the file and keep the original license file next to it (as `app/src-tauri/windows/installer.nsi` does). When adding a direct dependency, add an entry to the in-app list in `app/src/components/settings/aboutData.ts`
9. Launching the game accepts only an online Java Edition entitlement obtained through Lumi's own Microsoft application, and the official profile of the same account. The desktop app, shortcuts and the command line share the core launch authorization boundary. Offline usernames, third-party authentication servers and authentication injectors are not accepted
10. No `unsafe` (`unsafe_code = "forbid"` across the workspace); system APIs are used only through existing safe wrapper crates. The single exception is `crates/winmem`: each `unsafe` block contains exactly one call and must carry a `// SAFETY:` comment explaining why it is sound (enforced by clippy). It is used only by the app shell and the command-line tool; the core library does not depend on it

## 9. Error Handling

- No `unwrap()` / `expect()` in non-test Rust code (clippy reports them); when truly necessary, use `#[allow]` with a written reason
- No empty `catch` and no swallowed errors — log at least one line
- The frontend catches three things: global errors, unhandled promise rejections and render errors (`ErrorBoundary`). **Never a blank screen**
- Errors shown to users must be in plain language, with a "next step" button whenever one makes sense

## 10. Required Checks Before Committing

```powershell
pnpm check
```

It runs, in order: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm typecheck`, `pnpm lint` and `pnpm test`. If you changed the UI, also run `pnpm --dir app build` followed by `pnpm check:bundle`.

After changing the core library or commands, start the app once with `pnpm dev`; changes that affect launching the game must actually be tested by entering a game.

## 11. Dependencies

- Every new dependency must pass three questions: is there something lighter? Is it actively maintained? Is its license compatible with `GPL-3.0-only`?
- No second component library or animation library (animation uses Motion only)
- Pre-release dependencies (`specta`, `tauri-specta`, `specta-typescript`) must be pinned to exact versions with `=`

## 12. Performance Budget

Cold start to first screen < 0.8 s; click feedback < 100 ms; animations at a steady 60 fps; first-screen JS ≤ 250 KiB gzipped (enforced in CI by `pnpm check:bundle`); at most one long task (> 50 ms) on the first screen. Routes are lazy-loaded and icons are imported individually.
