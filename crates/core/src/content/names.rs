//! 中文名。
//!
//! 两个来源，按这个顺序用：
//! 1. 我们自己的中文名表 `crates/core/data/zh-names.json`（CC0，人工核对过）。表里还记着两个平台上
//!    **确认是同一个项目**的编号（生成时用同一个文件的 SHA-1 对上的），资源中心合并结果用。
//! 2. 模组自带的简体中文语言文件：在英文语言文件里找值正好是模组名的键（创造物品栏、Mod Menu 的
//!    名字翻译键……），取同一个键的中文值。读的是玩家自己装的文件，不分发任何数据。
//!
//! 中文名只用来显示和搜索，**不参与「是不是同一个项目」的判断**。

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::ContentSource;
use super::identify::{IdentityCache, IdentitySource, first_mods_table};
use super::modrinth::ProjectHit;

/// 语言文件最多读多少（再大就不是正常的语言文件了）
const MAX_LANG_BYTES: u64 = 4 * 1024 * 1024;
/// 中文名最多几个字（再长多半是一句话，不是名字）
const MAX_ZH_CHARS: usize = 24;
/// 一个压缩包最多看几个命名空间的语言文件
const MAX_NAMESPACES: usize = 12;

// ---------- 从模组文件里认 ----------

type Lang = Map<String, Value>;

/// 从模组压缩包里认中文名。`known` 是已经知道的英文名（平台上的标题），
/// 和文件里自己写的显示名一起拿去和英文语言文件比。认不出返回 `None`。
pub fn zh_name_in_archive<R: Read + Seek>(reader: R, known: &[&str]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(reader).ok()?;
    let info = mod_info(&mut archive);
    let mut names: Vec<String> = known.iter().map(|s| (*s).to_owned()).collect();
    names.extend(info.names);

    // 命名空间 → (英文, 中文) 语言文件在压缩包里的名字
    let mut files: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    let all: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.into_owned())
        .collect();
    for name in all {
        let Some((namespace, lang)) = lang_file(&name) else {
            continue;
        };
        let slot = files.entry(namespace.to_owned()).or_default();
        match lang.as_str() {
            "en_us" => slot.0 = Some(name.clone()),
            "zh_cn" => slot.1 = Some(name.clone()),
            _ => {}
        }
    }
    // 模组自己的命名空间先看
    let mut order: Vec<(String, String, String)> = files
        .into_iter()
        .filter_map(|(ns, (en, zh))| Some((ns, en?, zh?)))
        .collect();
    order.sort_by_key(|(ns, _, _)| !info.ids.iter().any(|id| id == ns));
    order.truncate(MAX_NAMESPACES);

    let mut votes: BTreeMap<String, u32> = BTreeMap::new();
    for (_, en_name, zh_name) in order {
        let (Some(en), Some(zh)) = (
            read_lang(&mut archive, &en_name),
            read_lang(&mut archive, &zh_name),
        ) else {
            continue;
        };
        if let Some(name) = translation_key(&zh, &info.ids) {
            return tidy(&name, &names);
        }
        vote(&en, &zh, &names, &mut votes);
    }
    best(votes).and_then(|name| tidy(&name, &names))
}

/// 最后再去一次前后带的英文原名（「Chat Tools [聊天工具箱]」），再清理一次
fn tidy(name: &str, names: &[String]) -> Option<String> {
    let wanted: Vec<String> = names.iter().map(|n| loose(n)).collect();
    clean(&without_english(name, &wanted))
}

/// 文件里写的模组编号和显示名
#[derive(Default)]
struct ModInfo {
    ids: Vec<String>,
    names: Vec<String>,
}

fn mod_info<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> ModInfo {
    let mut info = ModInfo::default();
    for toml in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(text) = read_small(archive, toml) {
            let fields = first_mods_table(&String::from_utf8_lossy(&text));
            info.ids.extend(fields.get("modId").cloned());
            info.names.extend(fields.get("displayName").cloned());
            return info;
        }
    }
    let json = |bytes: Vec<u8>| serde_json::from_slice::<Value>(strip_bom(&bytes)).ok();
    if let Some(value) = read_small(archive, "fabric.mod.json").and_then(json) {
        info.ids.extend(value["id"].as_str().map(str::to_owned));
        info.names.extend(value["name"].as_str().map(str::to_owned));
    } else if let Some(value) = read_small(archive, "quilt.mod.json").and_then(json) {
        let loader = &value["quilt_loader"];
        info.ids.extend(loader["id"].as_str().map(str::to_owned));
        info.names
            .extend(loader["metadata"]["name"].as_str().map(str::to_owned));
    } else if let Some(value) = read_small(archive, "mcmod.info").and_then(json) {
        // 1.12 及以前：[{"modid": "...", "name": "..."}]
        let first = value.as_array().and_then(|a| a.first()).unwrap_or(&value);
        info.ids.extend(first["modid"].as_str().map(str::to_owned));
        info.names.extend(first["name"].as_str().map(str::to_owned));
    }
    info
}

/// `assets/<命名空间>/lang/<语言>.json`（1.12 及以前是 `.lang`）→ (命名空间, 小写的语言)
fn lang_file(name: &str) -> Option<(&str, String)> {
    let rest = name.strip_prefix("assets/")?;
    let (namespace, rest) = rest.split_once('/')?;
    let file = rest.strip_prefix("lang/")?;
    if file.contains('/') {
        return None;
    }
    let lang = file
        .strip_suffix(".json")
        .or_else(|| file.strip_suffix(".lang"))?;
    Some((namespace, lang.to_ascii_lowercase()))
}

fn read_small<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let entry = archive.by_name(name).ok()?;
    if entry.size() > MAX_LANG_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry.take(MAX_LANG_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// 读一个语言文件：JSON，或者老版本的「键=值」
fn read_lang<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<Lang> {
    let bytes = read_small(archive, name)?;
    let bytes = strip_bom(&bytes);
    if name.ends_with(".json") {
        return serde_json::from_slice::<Lang>(bytes).ok();
    }
    let text = String::from_utf8_lossy(bytes);
    Some(
        text.lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), Value::String(v.trim().to_owned())))
            .collect(),
    )
}

/// 专门给模组名用的翻译键（Mod Menu、NeoForge 模组列表）：有就直接用
fn translation_key(zh: &Lang, ids: &[String]) -> Option<String> {
    ids.iter().find_map(|id| {
        let keys = [
            format!("modmenu.nameTranslation.{id}"),
            format!("fml.menu.mods.info.displayname.{id}"),
        ];
        zh.iter()
            .find(|(k, _)| keys.iter().any(|key| key.eq_ignore_ascii_case(k)))
            .and_then(|(_, v)| v.as_str())
            .and_then(clean)
    })
}

/// 英文值正好是模组名的键，拿同一个键的中文值投票；创造物品栏的键分量重
/// （同名的普通按钮，比如 Create 的「创建」，就比不过「机械动力」）
fn vote(en: &Lang, zh: &Lang, names: &[String], votes: &mut BTreeMap<String, u32>) {
    let wanted: Vec<String> = names
        .iter()
        .map(|n| loose(n))
        .filter(|n| n.chars().count() >= 2)
        .collect();
    if wanted.is_empty() {
        return;
    }
    for (key, value) in en {
        let Some(value) = value.as_str() else {
            continue;
        };
        let Some(zh_value) = zh.get(key).and_then(Value::as_str) else {
            continue;
        };
        let tab = key.to_ascii_lowercase().starts_with("itemgroup.");
        if wanted.contains(&loose(value)) {
            if let Some(translated) = clean(&without_english(zh_value, &wanted)) {
                *votes.entry(translated).or_default() += if tab { 3 } else { 1 };
            }
        } else if tab
            // 「Ice And Fire | Armors」↔「冰火传说丨装备」：分隔符前面正好是模组名
            && let (Some(en_head), Some(zh_head)) = (head(value), head(zh_value))
            && wanted.contains(&loose(en_head))
            && let Some(translated) = clean(zh_head)
        {
            *votes.entry(translated).or_default() += 2;
        }
    }
}

/// 中文名前后带的英文原名去掉：「苹果皮 AppleSkin」「精巧手艺Handcrafted」（后面的，有没有空格都算）、
/// 「AppleSkin 苹果皮」「Chat Tools [聊天工具箱]」（前面的，要有空格，剩下的至少三个字：
/// 「Auth Me 设置」这种剩下「设置」两个字的不算）
fn without_english(value: &str, wanted: &[String]) -> String {
    let value = value.trim();
    let is_latin = |c: char| c.is_ascii() || c == '’';
    // 结尾那段英文
    let tail_start = value
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_latin(*c))
        .last()
        .map_or(value.len(), |(i, _)| i);
    let (head, tail) = value.split_at(tail_start);
    if !tail.is_empty() && wanted.contains(&loose(tail)) && has_cjk(head) {
        return head.trim().to_owned();
    }
    if let Some((head, tail)) = split_latin_head(value)
        && wanted.contains(&loose(head))
        && has_cjk(tail)
        && tail.trim().chars().count() >= 3
    {
        return tail.trim().to_owned();
    }
    value.to_owned()
}

/// 开头那段英文（到最后一个空格为止）和后面剩下的
fn split_latin_head(value: &str) -> Option<(&str, &str)> {
    let first_cjk = value.char_indices().find(|(_, c)| !c.is_ascii())?.0;
    let cut = value[..first_cjk].rfind(' ')?;
    Some((&value[..cut], &value[cut..]))
}

/// 分隔符（「|」「丨」「:」「：」「 - 」）前面那段；没有分隔符返回 `None`
fn head(value: &str) -> Option<&str> {
    let cut = ['|', '丨', ':', '：']
        .iter()
        .filter_map(|sep| value.find(*sep))
        .chain(value.find(" - "))
        .min()?;
    let head = value[..cut].trim();
    (!head.is_empty()).then_some(head)
}

fn best(votes: BTreeMap<String, u32>) -> Option<String> {
    let mut best: Option<(String, u32)> = None;
    for (name, count) in votes {
        if best.as_ref().is_none_or(|(_, top)| count > *top) {
            best = Some((name, count));
        }
    }
    best.map(|(name, _)| name)
}

/// 去掉格式代码和首尾空白；不带汉字、太长、带占位符的不算名字
fn clean(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
            continue;
        }
        out.push(c);
    }
    // 前后的装饰符号（「- 像素宝可梦」「▒ 模糊」）
    let mut out = out.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '-' | '–' | '—' | '·' | '•' | '▒' | '|' | '/' | '\\' | '*' | '~' | '#'
            )
    });
    // 整个包在括号里的（「[夸克]」）
    for (open, close) in [('[', ']'), ('【', '】'), ('(', ')'), ('（', '）')] {
        if let Some(inner) = out.strip_prefix(open).and_then(|o| o.strip_suffix(close))
            && !inner.contains([open, close])
        {
            out = inner.trim();
        }
    }
    // 「合成辅助(CraftingTweaks)」：结尾括号里只有英文（就是原名）的去掉
    if let Some(open) = out
        .ends_with([')', '）', ']', '】'])
        .then(|| out.rfind(['(', '（', '[', '【']))
        .flatten()
    {
        let (name, note) = out.split_at(open);
        if has_cjk(name) && !has_cjk(note) {
            out = name.trim_end();
        }
    }
    // 「时间漩涡模组」：结尾的「模组」两个字不算名字
    if let Some(name) = out.strip_suffix("模组")
        && name.chars().count() >= 2
    {
        out = name.trim_end();
    }
    let ok = has_cjk(out)
        && out.chars().count() <= MAX_ZH_CHARS
        && !out.contains('%')
        && !out.contains('\n')
        && !out.contains("{0}");
    ok.then(|| out.to_owned())
}

/// 比名字用：小写，去掉格式代码，只留字母和数字
fn loose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
            continue;
        }
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// 有没有汉字
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2A6DF}')
    })
}

/// 从模组文件认中文名（文件打不开、不是压缩包都返回 `None`）
pub fn zh_name_in_file(file: &Path, known: &[&str]) -> Option<String> {
    let handle = std::fs::File::open(file).ok()?;
    zh_name_in_archive(std::io::BufReader::new(handle), known)
}

// ---------- 中文名表 ----------

/// 中文名从哪来的
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NameFrom {
    /// 模组自带的简体中文语言文件
    Mod,
    /// 我们自己翻的
    Manual,
}

/// 表里的一条
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameEntry {
    pub zh: String,
    /// 平台上的英文名（中文搜索时拿它去平台搜）
    pub en: String,
    /// Modrinth 项目编号
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mr: Option<String>,
    /// CurseForge 项目编号
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf: Option<u32>,
    /// 玩家常用的别名、简称（也能搜到）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub from: NameFrom,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NameFile {
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub license: String,
    /// 按两个平台的下载量从高到低排（搜索同分时下载量高的在前）
    #[serde(default)]
    pub entries: Vec<NameEntry>,
    /// 没有中文名、但确认过两个平台上是同一个项目的：[Modrinth 编号, CurseForge 编号]
    #[serde(default)]
    pub links: Vec<(String, u32)>,
}

/// 读进内存的中文名表
#[derive(Debug, Default)]
pub struct NameTable {
    entries: Vec<NameEntry>,
    by_modrinth: HashMap<String, usize>,
    by_curseforge: HashMap<u32, usize>,
    /// 只有关联、没有中文名的
    links_mr: HashMap<String, u32>,
    links_cf: HashMap<u32, String>,
}

const TABLE_JSON: &str = include_str!("../../data/zh-names.json");

/// 程序自带的中文名表（第一次用时解析）
pub fn table() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| match NameTable::parse(TABLE_JSON) {
        Ok(table) => table,
        Err(error) => {
            tracing::error!(%error, "中文名表格式不对，先不显示中文名");
            NameTable::default()
        }
    })
}

/// 中文搜索的一个结果
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct NameMatch {
    pub zh: String,
    pub en: String,
    pub modrinth: Option<String>,
    pub curseforge: Option<String>,
}

impl NameTable {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let file: NameFile = serde_json::from_str(json)?;
        let mut table = Self::new(file.entries);
        for (mr, cf) in file.links {
            table.links_mr.entry(mr.clone()).or_insert(cf);
            table.links_cf.entry(cf).or_insert(mr);
        }
        Ok(table)
    }

    pub fn new(entries: Vec<NameEntry>) -> Self {
        let mut table = Self {
            entries,
            ..Self::default()
        };
        for (i, entry) in table.entries.iter().enumerate() {
            if let Some(id) = &entry.mr {
                table.by_modrinth.entry(id.clone()).or_insert(i);
            }
            if let Some(id) = entry.cf {
                table.by_curseforge.entry(id).or_insert(i);
            }
        }
        table
    }

    pub fn entries(&self) -> &[NameEntry] {
        &self.entries
    }

    fn entry(&self, source: ContentSource, id: &str) -> Option<&NameEntry> {
        let index = match source {
            ContentSource::Modrinth => self.by_modrinth.get(id),
            ContentSource::CurseForge => id
                .parse::<u32>()
                .ok()
                .and_then(|n| self.by_curseforge.get(&n)),
        }?;
        self.entries.get(*index)
    }

    pub fn zh_for(&self, source: ContentSource, id: &str) -> Option<&str> {
        self.entry(source, id).map(|e| e.zh.as_str())
    }

    /// 这个项目在另一个平台上的编号（表里确认过的）
    pub fn twin(&self, source: ContentSource, id: &str) -> Option<String> {
        let from_entry = self.entry(source, id).and_then(|entry| match source {
            ContentSource::Modrinth => entry.cf.map(|n| n.to_string()),
            ContentSource::CurseForge => entry.mr.clone(),
        });
        from_entry.or_else(|| match source {
            ContentSource::Modrinth => self.links_mr.get(id).map(|n| n.to_string()),
            ContentSource::CurseForge => id
                .parse::<u32>()
                .ok()
                .and_then(|n| self.links_cf.get(&n).cloned()),
        })
    }

    /// 只有关联、没有中文名的那些
    pub fn links(&self) -> impl Iterator<Item = (&String, &u32)> {
        self.links_mr.iter()
    }

    /// 中文关键词查表：名字完全一样 > 开头一样 > 包含 > 关键词里包含整个名字；同分的下载量高的在前
    pub fn search(&self, query: &str, limit: usize) -> Vec<NameMatch> {
        let wanted = loose(query);
        if !has_cjk(&wanted) {
            return Vec::new();
        }
        let mut scored: Vec<(u8, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(rank, entry)| {
                let score = std::iter::once(&entry.zh)
                    .chain(&entry.aliases)
                    .map(|name| score(&loose(name), &wanted))
                    .max()
                    .unwrap_or(0);
                (score > 0).then_some((score, rank))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        // 同一个模组两个平台各一条（没确认过是同一个）时，英文名一样的合成一个
        let mut out: Vec<NameMatch> = Vec::new();
        for entry in scored
            .into_iter()
            .filter_map(|(_, rank)| self.entries.get(rank))
        {
            let cf = entry.cf.map(|n| n.to_string());
            if let Some(same) = out.iter_mut().find(|m| loose(&m.en) == loose(&entry.en)) {
                same.modrinth = same.modrinth.take().or_else(|| entry.mr.clone());
                same.curseforge = same.curseforge.take().or(cf);
                continue;
            }
            if out.len() == limit {
                break;
            }
            out.push(NameMatch {
                zh: entry.zh.clone(),
                en: entry.en.clone(),
                modrinth: entry.mr.clone(),
                curseforge: cf,
            });
        }
        out
    }
}

fn score(name: &str, wanted: &str) -> u8 {
    if name.is_empty() {
        0
    } else if name == wanted {
        4
    } else if name.starts_with(wanted) {
        3
    } else if name.contains(wanted) {
        2
    } else if wanted.contains(name) && name.chars().count() >= 2 {
        1
    } else {
        0
    }
}

// ---------- 从识别缓存里学到的 ----------

type Key = (ContentSource, String);

/// 装过的文件教会我们的：模组自带的中文名、同一个文件证明的两个平台的关联
#[derive(Debug, Default)]
pub struct Learned {
    zh: HashMap<Key, String>,
    twins: HashMap<Key, String>,
}

impl Learned {
    pub fn from_cache(cache: &IdentityCache) -> Self {
        let mut learned = Self::default();
        for (sha1, identity) in cache.identities() {
            let modrinth = (identity.source == IdentitySource::Modrinth)
                .then(|| identity.project_id.clone())
                .flatten();
            let curseforge = cache.curseforge_project(sha1).map(|n| n.to_string());
            if let (Some(mr), Some(cf)) = (&modrinth, &curseforge) {
                learned
                    .twins
                    .insert((ContentSource::Modrinth, mr.clone()), cf.clone());
                learned
                    .twins
                    .insert((ContentSource::CurseForge, cf.clone()), mr.clone());
            }
            if let Some(zh) = cache.zh_name(sha1) {
                if let Some(mr) = modrinth {
                    learned
                        .zh
                        .insert((ContentSource::Modrinth, mr), zh.to_owned());
                }
                if let Some(cf) = curseforge {
                    learned
                        .zh
                        .insert((ContentSource::CurseForge, cf), zh.to_owned());
                }
            }
        }
        learned
    }

    /// 另一个平台上的编号：先看表，再看装过的文件
    pub fn twin(&self, source: ContentSource, id: &str) -> Option<String> {
        table()
            .twin(source, id)
            .or_else(|| self.twins.get(&(source, id.to_owned())).cloned())
    }

    /// 中文名：先看表（也看另一个平台的编号），再看装过的文件自带的
    pub fn zh(&self, source: ContentSource, id: &str) -> Option<String> {
        let other = |s: ContentSource| match s {
            ContentSource::Modrinth => ContentSource::CurseForge,
            ContentSource::CurseForge => ContentSource::Modrinth,
        };
        let twin = self.twin(source, id);
        table()
            .zh_for(source, id)
            .or_else(|| {
                twin.as_deref()
                    .and_then(|t| table().zh_for(other(source), t))
            })
            .map(str::to_owned)
            .or_else(|| self.zh.get(&(source, id.to_owned())).cloned())
            .or_else(|| twin.and_then(|t| self.zh.get(&(other(source), t)).cloned()))
    }

    /// 给搜索结果补上中文名、另一个平台上的编号
    pub fn annotate(&self, hits: &mut [ProjectHit]) {
        for hit in hits {
            hit.title_zh = self.zh(hit.source, &hit.project_id);
            hit.twin_id = self.twin(hit.source, &hit.project_id);
        }
    }
}

/// 按游戏目录记住 [`Learned`]：识别缓存文件没变就不重新读（每次搜索都要用）
#[derive(Debug, Default)]
pub struct NameBook {
    inner: Mutex<Option<BookEntry>>,
}

/// 记住的是哪个仓库、识别缓存当时的修改时间、读出来的结果
type BookEntry = (PathBuf, Option<SystemTime>, Arc<Learned>);

impl NameBook {
    pub fn learned(&self, store_root: &Path) -> Arc<Learned> {
        let modified = std::fs::metadata(store_root.join(super::identify::CACHE_FILE))
            .and_then(|m| m.modified())
            .ok();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((root, at, learned)) = inner.as_ref()
            && root == store_root
            && *at == modified
        {
            return learned.clone();
        }
        let learned = Arc::new(Learned::from_cache(&IdentityCache::load(store_root)));
        *inner = Some((store_root.to_owned(), modified, learned.clone()));
        learned
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use super::*;

    fn jar(entries: &[(&str, &str)]) -> Cursor<Vec<u8>> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        let mut out = zip.finish().unwrap();
        out.set_position(0);
        out
    }

    const CREATE_TOML: &str = "[[mods]]\nmodId=\"create\"\ndisplayName=\"Create\"\n";

    #[test]
    fn creative_tab_beats_a_button_with_the_same_word() {
        let file = jar(&[
            ("META-INF/neoforge.mods.toml", CREATE_TOML),
            (
                "assets/create/lang/en_us.json",
                r#"{"itemGroup.create.base":"Create","itemGroup.create.palettes":"Create's Building Blocks","gui.schematic.create":"Create"}"#,
            ),
            (
                "assets/create/lang/zh_cn.json",
                r#"{"itemGroup.create.base":"§6机械动力","itemGroup.create.palettes":"机械动力丨建筑方块","gui.schematic.create":"创建"}"#,
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]).as_deref(), Some("机械动力"));
    }

    #[test]
    fn tab_prefix_and_tidy_names() {
        let file = jar(&[
            (
                "META-INF/neoforge.mods.toml",
                "[[mods]]\nmodId=\"iceandfire\" #mandatory\ndisplayName=\"Ice And Fire\"\n",
            ),
            (
                "assets/iceandfire/lang/en_us.json",
                r#"{"itemGroup.iceandfire.armors":"Ice And Fire | Armors","itemGroup.iceandfire.items":"Ice And Fire | Items"}"#,
            ),
            (
                "assets/iceandfire/lang/zh_cn.json",
                r#"{"itemGroup.iceandfire.armors":"冰火传说丨装备","itemGroup.iceandfire.items":"冰火传说丨物品"}"#,
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]).as_deref(), Some("冰火传说"));
        assert_eq!(
            clean("合成辅助(CraftingTweaks)").as_deref(),
            Some("合成辅助")
        );
        assert_eq!(clean("自由视角（Freecam）").as_deref(), Some("自由视角"));
        assert_eq!(clean("时间漩涡模组").as_deref(), Some("时间漩涡"));
        assert_eq!(clean("模组").as_deref(), Some("模组"), "只有两个字时不去");
        assert_eq!(
            clean("机械动力（附属）").as_deref(),
            Some("机械动力（附属）")
        );
    }

    #[test]
    fn translation_key_values_are_tidied_too() {
        let file = jar(&[
            (
                "fabric.mod.json",
                r#"{"id":"chattools","name":"Chat Tools","version":"1"}"#,
            ),
            ("assets/chattools/lang/en_us.json", r#"{"a":"b"}"#),
            (
                "assets/chattools/lang/zh_cn.json",
                r#"{"modmenu.nameTranslation.chattools":"Chat Tools [聊天工具箱]"}"#,
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]).as_deref(), Some("聊天工具箱"));
    }

    #[test]
    fn mod_menu_translation_key_wins() {
        let file = jar(&[
            (
                "fabric.mod.json",
                r#"{"id":"sodium","name":"Sodium","version":"1"}"#,
            ),
            ("assets/sodium/lang/en_us.json", r#"{"a":"b"}"#),
            (
                "assets/sodium/lang/zh_cn.json",
                r#"{"modmenu.nameTranslation.sodium":"钠"}"#,
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]).as_deref(), Some("钠"));
    }

    #[test]
    fn platform_title_also_counts() {
        // 文件里的显示名是「waystones」，平台上叫「Waystones」：大小写不同也算
        let file = jar(&[
            (
                "META-INF/mods.toml",
                "[[mods]]\nmodId=\"waystones\"\ndisplayName=\"x\"\n",
            ),
            (
                "assets/waystones/lang/en_us.json",
                r#"{"itemGroup.waystones":"Waystones"}"#,
            ),
            (
                "assets/waystones/lang/zh_cn.json",
                "\u{FEFF}{\"itemGroup.waystones\":\"传送石碑\"}",
            ),
        ]);
        assert_eq!(
            zh_name_in_archive(file, &["Waystones"]).as_deref(),
            Some("传送石碑")
        );
    }

    #[test]
    fn untranslated_or_sentence_values_are_ignored() {
        let file = jar(&[
            ("META-INF/neoforge.mods.toml", CREATE_TOML),
            (
                "assets/create/lang/en_us.json",
                r#"{"itemGroup.create":"Create","k2":"Create"}"#,
            ),
            (
                "assets/create/lang/zh_cn.json",
                r#"{"itemGroup.create":"Create","k2":"这是一个非常非常非常非常长的一整句话而不是一个模组的名字吧"}"#,
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]), None);
    }

    #[test]
    fn old_lang_files_work() {
        let file = jar(&[
            (
                "mcmod.info",
                r#"[{"modid":"jei","name":"Just Enough Items"}]"#,
            ),
            (
                "assets/jei/lang/en_US.lang",
                "itemGroup.jei=Just Enough Items\n",
            ),
            (
                "assets/jei/lang/zh_CN.lang",
                "# 注释\nitemGroup.jei=物品管理器\n",
            ),
        ]);
        assert_eq!(zh_name_in_archive(file, &[]).as_deref(), Some("物品管理器"));
    }

    fn entry(zh: &str, en: &str, mr: Option<&str>, cf: Option<u32>) -> NameEntry {
        NameEntry {
            zh: zh.to_owned(),
            en: en.to_owned(),
            mr: mr.map(str::to_owned),
            cf,
            aliases: Vec::new(),
            from: NameFrom::Manual,
        }
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_contains() {
        let table = NameTable::new(vec![
            entry("通用机械", "Mekanism", Some("m"), None),
            entry(
                "机械动力：蒸汽与铁路",
                "Create: Steam 'n' Rails",
                Some("r"),
                None,
            ),
            entry("机械动力", "Create", Some("c"), Some(1)),
            entry("钠", "Sodium", Some("s"), None),
        ]);
        let found: Vec<String> = table.search("机械", 10).into_iter().map(|m| m.en).collect();
        // 开头一样的两个按表里的顺序（下载量），包含的排后面
        assert_eq!(found, ["Create: Steam 'n' Rails", "Create", "Mekanism"]);
        assert_eq!(table.search("机械动力", 1)[0].en, "Create");
        assert_eq!(table.search(" 钠 ", 5)[0].en, "Sodium");
        assert_eq!(
            table.search("机械动力模组", 5)[0].en,
            "Create",
            "关键词里包含整个名字"
        );
        assert!(table.search("sodium", 5).is_empty(), "不带汉字不查表");
        assert_eq!(
            table.twin(ContentSource::CurseForge, "1").as_deref(),
            Some("c")
        );
        assert_eq!(
            table.twin(ContentSource::Modrinth, "c").as_deref(),
            Some("1")
        );
        assert_eq!(
            table.zh_for(ContentSource::CurseForge, "1"),
            Some("机械动力")
        );
    }

    #[test]
    fn bundled_table_is_sane() {
        let table = NameTable::parse(TABLE_JSON).expect("中文名表要能解析");
        let mut mr_seen = std::collections::HashSet::new();
        let mut cf_seen = std::collections::HashSet::new();
        for e in table.entries() {
            assert!(has_cjk(&e.zh), "{} 的中文名里没有汉字", e.en);
            assert!(!e.en.trim().is_empty());
            assert!(e.mr.is_some() || e.cf.is_some(), "{} 没有平台编号", e.en);
            assert!(e.zh.chars().count() <= MAX_ZH_CHARS, "{} 太长", e.zh);
            assert_eq!(e.zh.trim(), e.zh, "{} 首尾有空白", e.zh);
            if let Some(mr) = &e.mr {
                assert!(mr_seen.insert(mr.clone()), "Modrinth 编号重复：{mr}");
            }
            if let Some(cf) = e.cf {
                assert!(cf_seen.insert(cf), "CurseForge 编号重复：{cf}");
            }
        }
        for (mr, cf) in table.links() {
            assert!(!mr_seen.contains(mr), "关联和中文名重复：{mr}");
            assert!(!cf_seen.contains(cf), "关联和中文名重复：{cf}");
        }
    }

    #[test]
    fn english_name_inside_the_chinese_one_is_dropped() {
        let wanted = vec![loose("AppleSkin")];
        assert_eq!(without_english("苹果皮 AppleSkin", &wanted), "苹果皮");
        assert_eq!(without_english("AppleSkin 苹果皮", &wanted), "苹果皮");
        assert_eq!(without_english("Xaero 的小地图", &wanted), "Xaero 的小地图");
        let handcrafted = vec![loose("Handcrafted")];
        assert_eq!(
            without_english("精巧手艺Handcrafted", &handcrafted),
            "精巧手艺"
        );
        let chat = vec![loose("Chat Tools")];
        assert_eq!(
            clean(&without_english("Chat Tools [聊天工具箱]", &chat)).as_deref(),
            Some("聊天工具箱")
        );
        let auth = vec![loose("Auth Me")];
        assert_eq!(
            without_english("Auth Me 设置", &auth),
            "Auth Me 设置",
            "剩下两个字时不去"
        );
    }

    #[test]
    fn decorations_and_wrapping_brackets_are_trimmed() {
        assert_eq!(clean("[夸克]").as_deref(), Some("夸克"));
        assert_eq!(clean("- 像素宝可梦").as_deref(), Some("像素宝可梦"));
        assert_eq!(clean("▒ 模糊").as_deref(), Some("模糊"));
        assert_eq!(
            clean("雪！真实的魔法！").as_deref(),
            Some("雪！真实的魔法！")
        );
        assert_eq!(
            clean("[Let's Do]葡园酒香").as_deref(),
            Some("[Let's Do]葡园酒香")
        );
    }

    #[test]
    fn same_english_name_on_both_platforms_is_one_match() {
        let table = NameTable::new(vec![
            entry("机械动力", "Create", Some("c"), None),
            entry("机械动力", "Create", None, Some(1)),
        ]);
        let found = table.search("机械动力", 5);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].modrinth.as_deref(), Some("c"));
        assert_eq!(found[0].curseforge.as_deref(), Some("1"));
    }

    #[test]
    fn links_without_names_still_pair_projects() {
        let table =
            NameTable::parse(r#"{"format":1,"entries":[],"links":[["AANobbMI",394468]]}"#).unwrap();
        assert_eq!(
            table.twin(ContentSource::CurseForge, "394468").as_deref(),
            Some("AANobbMI")
        );
        assert_eq!(
            table.twin(ContentSource::Modrinth, "AANobbMI").as_deref(),
            Some("394468")
        );
        assert_eq!(table.zh_for(ContentSource::Modrinth, "AANobbMI"), None);
    }

    #[test]
    fn bundled_table_finds_the_acceptance_examples() {
        let table = table();
        assert_eq!(
            table.search("机械动力", 1).first().map(|m| m.en.as_str()),
            Some("Create")
        );
        assert_eq!(
            table.search("钠", 1).first().map(|m| m.en.as_str()),
            Some("Sodium")
        );
        // 验收时补的：玩家习惯叫 Jade「玉」
        assert_eq!(
            table.search("玉", 1).first().map(|m| m.en.as_str()),
            Some("Jade 🔍")
        );
        assert_eq!(
            table.search("铁氧体磁芯", 1).first().map(|m| m.en.as_str()),
            Some("FerriteCore")
        );
    }
}
