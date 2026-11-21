# Chinese Name Table `zh-names.json`

The table Lumi uses to show "Chinese name + English name" and to search for mods in Chinese.

## License

**CC0 1.0** (public domain): anyone may use it for any purpose without attribution.

The table contains **no** data from MC百科 (MCMod), PCL, HMCL or the CFPA translation packs.

## Where Each Entry Comes From

- `"from": "mod"`: the Simplified Chinese language file shipped inside the mod's own jar (`assets/<mod>/lang/zh_cn.json`).
  The builder looks for the key whose English value is exactly the mod's name (the creative tab or Mod Menu name key) and takes the Chinese value of the same key.
  Generated automatically by `mcl zh-names-build`, then reviewed by hand
- `"from": "manual"`: translated by us (popular mods that ship without a Chinese name). Kept as-is when the table is regenerated

## Fields

| Field | Meaning |
|---|---|
| `zh` | Chinese name |
| `en` | English name on the platform (used to search the platforms when the query is in Chinese) |
| `mr` | Modrinth project ID |
| `cf` | CurseForge project ID |
| `aliases` | Common nicknames and abbreviations used by players (also searchable); optional |
| `from` | See above |

`entries` are sorted by combined downloads on both platforms, highest first (when Chinese search scores tie, the more downloaded project comes first).

`links` are ID pairs `[Modrinth, CurseForge]` for projects without a Chinese name that have been **confirmed to be the same project on both platforms**:
the builder asks Modrinth about the SHA-1 of the latest CurseForge file, and only an identical file counts. Discover uses them when merging results from both platforms.

## Regenerating

```bash
mcl zh-names-build --top 1000 --report review.tsv
```

Requires a CurseForge API key (environment variable `MCL_CURSEFORGE_KEY`). Only the few files needed are read from each jar (HTTP range requests); whole jars are never downloaded.

## Fixing or Adding Names

Edit this file directly and submit the change. If you change a name that came from the mod itself, also change `from` to `manual`, otherwise the next regeneration will replace it.
`cargo test -p launcher-core names` checks the format (no duplicate IDs, Chinese names contain Chinese characters and are not too long).
