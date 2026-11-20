# 中文名表 `zh-names.json`

启动器显示「中文名 + 英文名」、用中文搜模组时用的表（D17e）。

## 许可

**CC0 1.0**（公共领域）：任何人、任何用途都能直接用，不用署名。

表里**没有**任何来自 MC百科、PCL、HMCL、CFPA 汉化包的数据。

## 每条从哪来

- `"from": "mod"`：模组自己 jar 里自带的简体中文语言文件（`assets/<模组>/lang/zh_cn.json`）。
  在英文语言文件里找值正好是模组名的键（创造物品栏、Mod Menu 的名字翻译键），取同一个键的中文值。
  由 `mcl zh-names-build` 自动认，再人工看过一遍
- `"from": "manual"`：我们自己翻的（模组没带中文名的热门模组）。重新生成时原样保留

## 字段

| 字段 | 意思 |
|---|---|
| `zh` | 中文名 |
| `en` | 平台上的英文名（中文搜索时拿它去平台搜） |
| `mr` | Modrinth 项目编号 |
| `cf` | CurseForge 项目编号 |
| `aliases` | 玩家常用的别名、简称（也能搜到），可以不写 |
| `from` | 见上 |

`entries` 按两个平台加起来的下载量从高到低排（中文搜索同分时下载量高的在前）。

`links` 是没有中文名、但**确认过两个平台上是同一个项目**的编号对 `[Modrinth, CurseForge]`：
生成时用 CurseForge 最新文件的 SHA-1 去问 Modrinth，同一个文件才算。资源中心合并两个平台的结果时用。

## 重新生成

```bash
mcl zh-names-build --top 1000 --report 核对表.tsv
```

要 CurseForge 的 Key（环境变量 `MCL_CURSEFORGE_KEY`）。只读 jar 里需要的几个文件（HTTP 分段请求），不下载整个 jar。

## 想改名字、补名字

直接改这个文件提交就行：改了模组自带的名字，记得把 `from` 改成 `manual`，不然下次重新生成会被换回去。
`cargo test -p launcher-core names` 会检查格式（编号不重复、中文名里有汉字、不太长）。
