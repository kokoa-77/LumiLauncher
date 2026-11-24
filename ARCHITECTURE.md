# Architecture

> For contributors and code reviewers: the repository layout, what each layer is responsible for, and the fixed steps for adding a feature. Contribution guidelines are in [CONTRIBUTING.md](CONTRIBUTING.md); design and engineering rules are in [docs/DESIGN.md](docs/DESIGN.md).

## Layout

```
.
├─ Cargo.toml              Rust workspace (core library, command-line tool, app shell)
├─ package.json            pnpm workspace entry: pnpm dev / pnpm build / pnpm check
├─ crates/
│  ├─ core/                Core library (pure Rust, no Tauri dependency)
│  │  ├─ src/
│  │  │  ├─ fs/            atomic.rs (atomic writes), safe_path.rs (safe path joining)
│  │  │  ├─ config/        settings.rs (reading and writing settings, corruption recovery, format migration)
│  │  │  ├─ jobs.rs        Job registry (deduplication, cancellation, progress throttling, automatic cleanup). Each job
│  │  │  │                 records its instance, source, every step, speed (network bytes only) and remaining time;
│  │  │  │                 download history is saved to jobs-history.json in the app data directory (written on a
│  │  │  │                 background thread, broken entries skipped), together with the app shell's "retry recipe"
│  │  │  ├─ redact.rs      Log redaction and the Secret wrapper
│  │  │  ├─ net/           http.rs (shared client, one User-Agent for every request), diagnostics.rs (parallel
│  │  │  │                 connectivity probe for core official endpoints), doh.rs (DNS-over-HTTPS via Cloudflare and
│  │  │  │                 AliDNS, steps aside when a proxy tool uses fake-IP mode), download.rs (concurrent,
│  │  │  │                 resumable, verified download engine), mclogs.rs (opt-in redacted log upload),
│  │  │  │                 status.rs (Minecraft server list ping: players online, latency, version, MOTD, icon),
│  │  │  │                 srv.rs (_minecraft._tcp SRV lookup against the system's DNS servers with a minimal DNS query)
│  │  │  ├─ meta/          manifest.rs (version manifest), version.rs (version JSON and inheritance merging),
│  │  │  │                 rules.rs (rule evaluation), maven.rs (Maven coordinates → paths)
│  │  │  ├─ paths.rs       GameDir: game directory layout — shared versions/ libraries/ assets/ (standard Mojang
│  │  │  │                 layout) + instances/ (every external name is validated)
│  │  │  ├─ java.rs        Java discovery (official runtimes, JAVA_HOME, PATH, registry, manually added) and selection
│  │  │  │                 by version; every Java has an ID
│  │  │  ├─ java/          runtime.rs: downloads Mojang's official runtime when no suitable Java exists (per-file
│  │  │  │                 verification, stored in <game dir>/.mcl-java/), removes downloaded runtimes;
│  │  │  │                 prefs.rs: user choices (manually added runtimes, default per major version, java.json)
│  │  │  ├─ auth/          Microsoft browser authorization (microsoft.rs), Java Edition entitlement and official profile;
│  │  │  │                 accounts.rs stores account summaries and cleans up legacy accounts, tokens.rs isolates
│  │  │  │                 tokens in Windows Credential Manager; the core launch authorization is bound to the current
│  │  │  │                 account, a revocation generation and an expiry; skin.rs caches official profile skins
│  │  │  ├─ process.rs     Shared child-process settings (no console window, arguments passed as an array)
│  │  │  ├─ capture.rs     Captures the game window for automatic world covers
│  │  │  ├─ install/       vanilla.rs: completes vanilla files (libraries, client, assets, natives, logging config);
│  │  │  │                 processors.rs: shared flow for Forge-family installers (libraries, processors, output
│  │  │  │                 checks, install receipt); neoforge.rs: NeoForge versions and installation;
│  │  │  │                 setup.rs: creating an instance (install vanilla / loader → complete files → register)
│  │  │  ├─ content/       mod.rs: instance content lists, enabling / disabling (renaming mods, writing Iris settings for
│  │  │  │                 shaders, writing options.txt for resource packs), identification, update checks, one-click
│  │  │  │                 updates, deletion, change history and undo;
│  │  │  │                 modrinth.rs / curseforge.rs: search, version selection, dependency resolution, Iris added
│  │  │  │                 automatically for shaders, hash-based identification and batch update checks;
│  │  │  │                 store.rs: content store (one copy per SHA-1; files are stored before being replaced or
│  │  │  │                 deleted, which makes undo possible);
│  │  │  │                 identify.rs: identification cache (file → hash → name / version / icon);
│  │  │  │                 names.rs: Chinese names (the bundled data/zh-names.json under CC0, names read from a mod's
│  │  │  │                 own zh_cn language file, Chinese keyword lookup);
│  │  │  │                 snapshot.rs: "never break your pack" snapshots (last working state / before a change),
│  │  │  │                 comparing changes after a crash, one-click restore;
│  │  │  │                 journal.rs: change journal — multi-file operations record each step and the original state
│  │  │  │                 first, and roll back on error or interruption;
│  │  │  │                 detail.rs: shared shape of project details; cache.rs: 10-minute in-memory cache for
│  │  │  │                 searches and details; favorites.rs: favorites (favorites.json);
│  │  │  │                 maps.rs: worlds from CurseForge (file selection by instance version, verified download,
│  │  │  │                 safe extraction into saves/ with safe_join, 4 GB / 200,000-entry limits, extract to a
│  │  │  │                 temporary directory and then rename)
│  │  │  ├─ instance/      Instances: instances/<name>/ (the instance folder is the game directory) +
│  │  │  │                 Lumi/instance.json (records the version to launch); listing, registration, unique naming,
│  │  │  │                 groups and pinning, copying, moving to the Recycle Bin, migrating the old layout;
│  │  │  │                 link.rs: linked versions from other launchers (versions and libraries are read in place;
│  │  │  │                 removing one deletes only Lumi's record)
│  │  │  ├─ external/      Other launchers: root.rs links another .minecraft in place (official / PCL / HMCL, game
│  │  │  │                 versions, loaders, version isolation, discovery on this computer); foreign.rs imports
│  │  │  │                 instances from Prism / MultiMC, CurseForge and ATLauncher (copies game files);
│  │  │  │                 modrinth_app.rs reads the Modrinth App database (SQLite) read-only
│  │  │  ├─ shortcut.rs    Desktop shortcuts: writes .lnk files from the public format (no PowerShell), converts world
│  │  │  │                 icons to rounded .ico files, parses the --play argument
│  │  │  ├─ modpack/       mrpack.rs: Modrinth modpack checks (paths, links, shortcuts, name collisions, domains, hashes)
│  │  │  │                 and import; remote.rs: installable modpack versions from Modrinth (Modrinth CDN only, SHA-1
│  │  │  │                 required); ready.rs: what is missing before import (game, loader, Java)
│  │  │  ├─ saves/         mod.rs: worlds, world info, screenshots; nbt.rs: NBT reading and writing (only writes what it
│  │  │  │                 fully understands); servers.rs: servers.dat (read, add with address validation,
│  │  │  │                 deduplication and atomic writes); backup.rs: world backups (zip files in Lumi/backups/,
│  │  │  │                 skipped when unchanged, retention for automatic backups, the current world is backed up
│  │  │  │                 before a restore); notes.rs: world notes (coordinates, to-dos, notes)
│  │  │  ├─ storage.rs     Storage analysis, safe cache cleanup and reference-aware cleanup of unused versions
│  │  │  ├─ sync.rs        Local instance sync (6 categories): local only, no cloud, instances stay independent
│  │  │  └─ launch/        args.rs (launch arguments, including Quick Play into a world or server), log.rs (log4j XML /
│  │  │                    plain-text log parsing), session.rs (game state machine), prepare.rs (pre-launch
│  │  │                    preparation: loader, file completion, Java, memory, arguments), memory.rs (automatic memory
│  │  │                    allocation and pre-launch checks), jvm.rs (GC presets, custom arguments split into an array
│  │  │                    with quote handling), gpu.rs (high-performance GPU preference: writes the Windows graphics
│  │  │                    setting and reverts only what Lumi wrote), mod.rs (starts the process, reads output, can
│  │  │                    force-stop)
│  │  ├─ data/          Bundled data: zh-names.json (Chinese names) and java/ (the SessionPipe bootstrap class)
│  │  └─ tests/         Integration tests: download.rs (local mock server), version_json.rs (real 1.21.1 data)
│  ├─ cli/                 mcl, the command-line debugging tool
│  └─ winmem/              System memory and window handle wrappers (the only crate allowed a few controlled `unsafe` calls)
├─ app/
│  ├─ src-tauri/           App shell
│  │  ├─ src/lib.rs        Startup, plugins, command registration, type export
│  │  ├─ src/commands.rs   App, settings and job commands
│  │  ├─ src/game.rs       Instance, launch and content search commands (background jobs + events)
│  │  ├─ src/instances.rs  Per-instance settings, modpack export, copying, deletion
│  │  ├─ src/accounts.rs   Account commands; the UI only receives limited summaries
│  │  ├─ src/linked.rs     Other launchers: linked directories (linked-roots.json, rescanned in the background), discovery,
│  │  │                    importing instances (copy → install game and loader)
│  │  ├─ src/discover.rs   Discover: project details, favorites, world search and install, server search / join / list
│  │  ├─ src/downloads.rs  Download center: history, retry recipes, DoH test, connectivity diagnostics, folders watched for manual downloads
│  │  ├─ src/claims.rs     Manual downloads for files whose authors disallow third-party downloads
│  │  ├─ src/drops.rs      Install by dragging files into the window
│  │  ├─ src/worlds.rs     Worlds: one instance's worlds, the latest world of each instance, screenshots
│  │  ├─ src/backups.rs    World backup commands and automatic backups
│  │  ├─ src/shortcuts.rs  Desktop shortcuts for instances and worlds
│  │  ├─ src/sync.rs       Local instance sync commands and launch / exit hooks
│  │  ├─ src/storage.rs    Storage usage, cache cleanup and unused version cleanup
│  │  ├─ src/system.rs     Memory (memory bar, pre-launch checks, memory cleanup via an elevated second process), Java, GPU
│  │  ├─ src/records.rs    A full record of every launch, shown in the instance's Logs tab
│  │  ├─ src/recovery.rs   Rolls back unfinished changes from the last run at startup
│  │  ├─ src/turns.rs      Only one write operation or game session per real game directory at a time
│  │  ├─ src/updates.rs    Signed updates for the installed version
│  │  ├─ src/installer.rs  Hand-off of the game directory chosen in the Windows installer
│  │  ├─ src/portable.rs   Portable mode, enabled by the Lumi.portable marker next to the executable
│  │  ├─ src/app_dirs.rs   Moves data from the pre-release app ID to com.lumilauncher.lumi
│  │  ├─ src/images.rs     Local image protocol (the UI only passes IDs; images are resized by ?w=)
│  │  ├─ src/sessions.rs   Running game sessions (state, last 5,000 log lines, stop switch)
│  │  ├─ src/events.rs     Backend → frontend events (job progress, session state, batched logs)
│  │  ├─ src/window.rs     Mica backdrop, show when ready, timeout fallback; window_lifecycle.rs tracks window rebuilds
│  │  ├─ src/logging.rs    Logging (daily rotation, redaction)
│  │  ├─ src/error.rs      The shared AppError format
│  │  ├─ tauri.conf.json   Window, CSP, bundling
│  │  ├─ capabilities/     UI permission allowlist
│  │  └─ windows/          NSIS installer template and hooks (derived from the Tauri template, see TAURI-LICENSE-MIT.txt)
│  └─ src/                 React UI
│     ├─ lib/bindings.ts   ★ Generated frontend ↔ backend interface (never edit by hand)
│     ├─ lib/              api.ts, report.ts, i18n.ts, motion.ts, theme.ts, themeEngine.ts, color.ts, layout.ts …
│     ├─ styles/           tokens.css (design tokens), global.css
│     ├─ components/       ui/ (Lumi's own component library), title bar, sidebar, error boundary, launch capsule, log drawer,
│     │                    task center, new instance flow, install confirmation, modpack import, change history,
│     │                    virtual lists; discover/, instance/, settings/, onboarding/, worlds/, account/ …
│     ├─ layouts/          AppShell (overall layout + page transitions)
│     ├─ pages/            Home, Instances, Instance detail, Discover, Project
│     ├─ stores/           Zustand stores (app.ts for settings; game.ts for instances, sessions, jobs and logs, subscribed to backend events)
│     ├─ locales/          zh-CN.json, en.json
│     └─ dev/              Dev-only component gallery and browser preview with fake data (not in release builds)
└─ scripts/                Release packaging, bundle size check and end-to-end test scripts
```

## Startup Sequence

1. Rust registers plugins (single instance → window state → opener) → setup: initialize logging → apply Mica to the main window (Windows 11) → create the job registry and event forwarding → load settings → start the "force show after 5 seconds" fallback
2. The window starts **hidden** (`visible: false`)
3. Frontend: set the theme from the system light / dark preference → call `appInfo` + `settingsGet` → render → call `appReady` two frames later
4. The backend shows the window only after `appReady`, so the first frame users see is the complete UI — no white flash
5. If the frontend has not reported ready within 5 seconds (for example, a script failed to load), the backend shows the window anyway and logs a warning

## Adding a Feature

Taking "add a command" as the example:

1. **Core library**: implement the feature and its unit tests in `crates/core` (no Tauri). Types exported to the frontend get
   `#[cfg_attr(feature = "specta", derive(specta::Type))]`
2. **Command line** (optional, recommended): add a subcommand to `crates/cli` and get it working there first
3. **Command**: write a thin function in `app/src-tauri/src/commands.rs` (or the matching domain file) with `#[tauri::command]` and
   `#[specta::specta]`; fallible commands return `Result<T, AppError>`, adding a `From` conversion in `error.rs` if needed
4. **Register**: add it to `collect_commands![...]` in `lib.rs`
5. **Generate types**: run `cargo test -p launcher-app`; `app/src/lib/bindings.ts` updates automatically
6. **UI**: call it through `commands.xxx()`, unwrap the result with `unwrap()`, pass errors to `reportError`; text goes into both
   `zh-CN.json` and `en.json`
7. **Check**: `pnpm check` must pass, then run the app once with `pnpm dev`

Events work the same way: define a struct with `#[derive(..., specta::Type, tauri_specta::Event)]` in `events.rs`, add it to `collect_events![...]`, and listen in the frontend with `events.xxx.listen(...)`.

## Data Locations

| Data | Location |
|---|---|
| Settings | `%APPDATA%\com.lumilauncher.lumi\settings.json` (data from the pre-release ID `com.unnamed.mclauncher` is moved here on first launch, see `app/src-tauri/src/app_dirs.rs`) |
| Logs | `%LOCALAPPDATA%\com.lumilauncher.lumi\logs\launcher.<date>.log` (kept for 7 days). The same folder also holds WebView2 data (UI preferences such as "Cards / List") |
| Game directory | `MCL_GAME_DIR` environment variable (development) > the directory chosen in Settings → Game directory > `%APPDATA%\com.lumilauncher.lumi\minecraft` |
| Portable mode | Everything above lives in the `LumiData` folder next to `Lumi.exe` when `Lumi.portable` sits beside it |
| Instances | `<game dir>\instances\<name>\` (the instance folder is the game directory). Lumi's own files are in its `Lumi\` subfolder: `instance.json` (instance info and the version to launch), `content.json` (where installed content came from), `history.json` (change history, up to 100 entries), `journal.json` (an unfinished change; normally absent, rolled back at startup), `backups/` + `backups.json` (world backups), `world-notes.json` (world notes) |
| Content store | `<game dir>\.mcl-store\`: `objects\<first two chars>\<SHA-1>` (replaced or deleted files), `identity.json` (identification cache), `icons\` (icons read from files) |
| Temporary files | `<game dir>\.mcl-tmp\` (same drive as the game directory, deleted after installation; the system temp directory is not used). Modpacks downloaded for online installation go to `.mcl-tmp\packs\` and are deleted afterwards; leftovers are cleaned at startup |
| Window position | Saved automatically by the window state plugin |
