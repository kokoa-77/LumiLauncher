//! 中文名表（D17e）：看本地模组认得怎么样、从两个平台的热门模组生成名表。
//!
//! 生成的办法：
//! 1. 两个平台各取下载量最高的若干个模组
//! 2. 用 CurseForge 最新文件的 SHA-1 去问 Modrinth：同一个文件 = 同一个项目（只认两边都唯一的）
//! 3. 每个项目只读 jar 里需要的几个文件（HTTP 分段请求：先读结尾的目录，再读语言文件），不下载整个 jar
//! 4. 用核心库同一套办法（[`names::zh_name_in_archive`]）认中文名
//!
//! 已有名表里「我们翻的」（`manual`）原样保留；「模组自带的」每次重新认。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use launcher_core::content::curseforge::{self as cf, CurseForge};
use launcher_core::content::modrinth;
use launcher_core::content::names::{self, NameEntry, NameFile, NameFrom};
use launcher_core::content::{ContentKind, SearchSort};
use launcher_core::fs::write_atomic;

/// 一次分段请求读多少
const BLOCK: u64 = 128 * 1024;
/// 同时读几个 jar
const PARALLEL: usize = 8;

/// 看一个文件夹里的模组各认出什么中文名（只读文件，不联网）
pub fn scan(dir: &Path) -> Result<()> {
    let mut files: Vec<_> = walk(dir)?
        .into_iter()
        .filter(|p| {
            p.extension().is_some_and(|e| {
                e.eq_ignore_ascii_case("jar") || e.eq_ignore_ascii_case("disabled")
            })
        })
        .collect();
    files.sort();
    let mut named = 0;
    for file in &files {
        let zh = names::zh_name_in_file(file, &[]);
        if zh.is_some() {
            named += 1;
        }
        println!(
            "{}\t{}",
            zh.as_deref().unwrap_or("-"),
            file.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        );
    }
    println!("共 {} 个文件，认出中文名 {named} 个", files.len());
    Ok(())
}

fn walk(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("读不了 {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(walk(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

/// 网络偶尔抖一下（TLS 握手断开之类）：同一个请求最多试 4 次
async fn retry<T, E: std::fmt::Display, Fut: std::future::Future<Output = Result<T, E>>>(
    what: &str,
    mut run: impl FnMut() -> Fut,
) -> Result<T> {
    let mut last = String::new();
    for attempt in 1..=4 {
        match run().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = error.to_string();
                eprintln!("{what}失败（第 {attempt} 次）：{last}");
                tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
            }
        }
    }
    bail!("{what}一直失败：{last}")
}

/// 一个候选项目（两个平台合在一起）
#[derive(Debug, Default, Clone)]
struct Project {
    en: String,
    mr: Option<String>,
    cf: Option<u32>,
    mr_downloads: f64,
    cf_downloads: f64,
    /// 读哪个文件认中文名：(地址, 是不是 CurseForge 的下载地址)
    file: Option<(String, bool)>,
}

impl Project {
    fn downloads(&self) -> f64 {
        self.mr_downloads + self.cf_downloads
    }
}

/// 从两个平台下载量前 `top` 的模组生成名表，写到 `out`；`report` 写一份核对用的表格
pub async fn build(top: u32, out: &Path, report: Option<&Path>) -> Result<()> {
    let client = launcher_core::net::build_client()?;
    let curseforge = CurseForge::from_env(client.clone());
    if !curseforge.has_key() {
        bail!("没有 CurseForge 的 Key（环境变量 MCL_CURSEFORGE_KEY）");
    }

    // 1. Modrinth 的热门模组（带最新版本编号）
    let mut mr_hits: Vec<(String, String, f64, Option<String>)> = Vec::new();
    let mut offset = 0;
    while offset < top {
        let url = format!(
            "https://api.modrinth.com/v2/search?facets=%5B%5B%22project_type%3Amod%22%5D%5D&index=downloads&limit=100&offset={offset}"
        );
        let body = retry("搜 Modrinth", || async {
            client
                .get(&url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await
        })
        .await?;
        let page: serde_json::Value = serde_json::from_slice(&body)?;
        let hits = page["hits"].as_array().cloned().unwrap_or_default();
        if hits.is_empty() {
            break;
        }
        for hit in hits {
            mr_hits.push((
                hit["project_id"].as_str().unwrap_or_default().to_owned(),
                hit["title"].as_str().unwrap_or_default().to_owned(),
                hit["downloads"].as_f64().unwrap_or(0.0),
                hit["latest_version"].as_str().map(str::to_owned),
            ));
        }
        offset += 100;
    }
    eprintln!("Modrinth：{} 个", mr_hits.len());

    // 2. CurseForge 的热门模组
    let mut cf_mods: Vec<cf::Mod> = Vec::new();
    let mut offset = 0;
    while offset < top {
        let query = cf::SearchQuery {
            text: "",
            class_id: cf::class_for(ContentKind::Mod),
            game_version: None,
            loader: None,
            sort: SearchSort::Downloads,
            category: None,
            offset,
            limit: 50,
        };
        let (mods, _, _) = retry("搜 CurseForge", || curseforge.search_mods(&query)).await?;
        if mods.is_empty() {
            break;
        }
        cf_mods.extend(mods);
        offset += 50;
    }
    eprintln!("CurseForge：{} 个", cf_mods.len());

    // 3. 同一个文件 = 同一个项目：CurseForge 文件的 SHA-1 问 Modrinth
    let mut hash_owner: HashMap<String, u32> = HashMap::new();
    for m in &cf_mods {
        for file in &m.latest_files {
            if let Some(sha1) = file.sha1() {
                hash_owner.insert(sha1.to_ascii_lowercase(), m.id);
            }
        }
    }
    let hashes: Vec<String> = hash_owner.keys().cloned().collect();
    let mut pairs: HashMap<u32, HashSet<String>> = HashMap::new();
    for chunk in hashes.chunks(400) {
        let found = retry("按 SHA-1 问 Modrinth", || {
            modrinth::identify_detailed(&client, chunk)
        })
        .await?;
        for (sha1, remote) in found {
            if let Some(cf_id) = hash_owner.get(&sha1.to_ascii_lowercase()) {
                pairs.entry(*cf_id).or_default().insert(remote.project_id);
            }
        }
    }
    // 两边都唯一才算
    let mut mr_to_cf: HashMap<String, Vec<u32>> = HashMap::new();
    for (cf_id, set) in &pairs {
        if set.len() == 1
            && let Some(mr) = set.iter().next()
        {
            mr_to_cf.entry(mr.clone()).or_default().push(*cf_id);
        }
    }
    let links: HashMap<u32, String> = mr_to_cf
        .into_iter()
        .filter(|(_, cfs)| cfs.len() == 1)
        .filter_map(|(mr, cfs)| cfs.first().map(|cf| (*cf, mr)))
        .collect();
    eprintln!("确认两个平台是同一个项目：{} 对", links.len());

    // 4. 合成项目列表
    let mut projects: Vec<Project> = Vec::new();
    let mut by_mr: HashMap<String, usize> = HashMap::new();
    let versions: Vec<String> = mr_hits.iter().filter_map(|h| h.3.clone()).collect();
    let mut primary: HashMap<String, String> = HashMap::new();
    for chunk in versions.chunks(80) {
        for version in retry("查 Modrinth 版本", || {
            modrinth::fetch_versions(&client, chunk)
        })
        .await?
        {
            if let Some(file) = version.primary_file() {
                primary.insert(version.project_id.clone(), file.url.clone());
            }
        }
    }
    for (id, title, downloads, _) in &mr_hits {
        by_mr.insert(id.clone(), projects.len());
        projects.push(Project {
            en: title.clone(),
            mr: Some(id.clone()),
            mr_downloads: *downloads,
            file: primary.get(id).map(|url| (url.clone(), false)),
            ..Project::default()
        });
    }
    for m in &cf_mods {
        let linked = links.get(&m.id).and_then(|mr| by_mr.get(mr).copied());
        let cf_file = m
            .latest_files
            .iter()
            .find_map(|f| f.usable_download_url().map(|u| (u.to_owned(), true)));
        match linked {
            Some(index) => {
                if let Some(p) = projects.get_mut(index) {
                    p.cf = Some(m.id);
                    p.cf_downloads = m.download_count;
                    if p.file.is_none() {
                        p.file = cf_file;
                    }
                }
            }
            None => {
                let mr = links.get(&m.id).cloned();
                projects.push(Project {
                    en: m.name.clone(),
                    mr,
                    cf: Some(m.id),
                    cf_downloads: m.download_count,
                    file: cf_file,
                    ..Project::default()
                });
            }
        }
    }
    projects.sort_by(|a, b| b.downloads().total_cmp(&a.downloads()));

    // 5. 读 jar 认中文名
    let key = curseforge.download_header();
    let key = key.map(|h| (h.name, h.value.expose().clone(), h.hosts));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut tasks = tokio::task::JoinSet::new();
    for (index, project) in projects.iter().enumerate() {
        let Some((url, _)) = project.file.clone() else {
            continue;
        };
        let permit = semaphore.clone().acquire_owned().await?;
        let client = client.clone();
        let title = project.en.clone();
        let key = key.clone();
        let rt = tokio::runtime::Handle::current();
        tasks.spawn_blocking(move || {
            let _permit = permit;
            let header = key.and_then(|(name, value, hosts)| {
                let host = launcher_core::net::url_host(&url)?;
                hosts.contains(&host.as_str()).then_some((name, value))
            });
            // CurseForge 的 edge 地址带分段请求头时直接 404，不带时 302 跳到真正的文件：
            // 先用 HEAD 跟着平台自己的跳转拿到最终地址（不自己拼地址）
            let url = match &header {
                Some((name, value)) => rt
                    .block_on(async {
                        client
                            .head(&url)
                            .header(*name, value.as_str())
                            .send()
                            .await
                            .map(|r| r.url().to_string())
                    })
                    .unwrap_or(url),
                None => url,
            };
            let fetch_once = |range: &str| -> std::io::Result<(u64, u64, Vec<u8>)> {
                rt.block_on(async {
                    let mut request = client.get(&url).header("Range", range);
                    if let Some((name, value)) = &header {
                        request = request.header(*name, value.as_str());
                    }
                    let response = request.send().await.map_err(std::io::Error::other)?;
                    let status = response.status().as_u16();
                    if status != 206 {
                        return Err(std::io::Error::other(format!(
                            "HTTP {status}（不支持分段）"
                        )));
                    }
                    let (start, total) = response
                        .headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .and_then(content_range)
                        .ok_or_else(|| std::io::Error::other("没有 Content-Range"))?;
                    let body = response.bytes().await.map_err(std::io::Error::other)?;
                    Ok((start, total, body.to_vec()))
                })
            };
            let fetch = |range: String| {
                let mut last = None;
                for attempt in 0..3 {
                    match fetch_once(&range) {
                        Ok(got) => return Ok(got),
                        Err(error) => {
                            last = Some(error);
                            std::thread::sleep(std::time::Duration::from_millis(
                                800 * (attempt + 1),
                            ));
                        }
                    }
                }
                Err(last.unwrap_or_else(|| std::io::Error::other("分段请求失败")))
            };
            let result = RangeReader::open(fetch)
                .map(|reader| names::zh_name_in_archive(reader, &[title.as_str()]));
            (index, result)
        });
    }
    let mut zh: BTreeMap<usize, String> = BTreeMap::new();
    let mut failed = 0;
    while let Some(done) = tasks.join_next().await {
        match done {
            Ok((index, Ok(Some(name)))) => {
                zh.insert(index, name);
            }
            Ok((_, Ok(None))) => {}
            Ok((index, Err(error))) => {
                failed += 1;
                let name = projects.get(index).map(|p| p.en.as_str()).unwrap_or("?");
                eprintln!("读不了 {name}：{error}");
            }
            Err(error) => {
                failed += 1;
                eprintln!("线程出错：{error}");
            }
        }
    }
    eprintln!(
        "{} 个项目，认出中文名 {} 个，读失败 {failed} 个",
        projects.len(),
        zh.len()
    );

    // 6. 合并已有名表里我们翻的
    let old: NameFile = match std::fs::read_to_string(out) {
        Ok(text) => serde_json::from_str(&text).context("已有名表格式不对")?,
        Err(_) => NameFile::default(),
    };
    let manual: Vec<NameEntry> = old
        .entries
        .into_iter()
        .filter(|e| e.from == NameFrom::Manual)
        .collect();
    let mut manual_left = manual.clone();
    let mut entries: Vec<NameEntry> = Vec::new();
    let mut table_links: Vec<(String, u32)> = Vec::new();
    for (index, p) in projects.iter().enumerate() {
        let taken = manual_left
            .iter()
            .position(|e| (e.mr.is_some() && e.mr == p.mr) || (e.cf.is_some() && e.cf == p.cf));
        if let Some(position) = taken {
            let mut entry = manual_left.remove(position);
            // 平台编号以这次查到的为准（补上另一个平台的）
            entry.mr = entry.mr.or_else(|| p.mr.clone());
            entry.cf = entry.cf.or(p.cf);
            entries.push(entry);
        } else if let Some(name) = zh.get(&index) {
            entries.push(NameEntry {
                zh: name.clone(),
                en: p.en.clone(),
                mr: p.mr.clone(),
                cf: p.cf,
                aliases: Vec::new(),
                from: NameFrom::Mod,
            });
        } else if let (Some(mr), Some(cf)) = (&p.mr, p.cf) {
            table_links.push((mr.clone(), cf));
        }
    }
    entries.extend(manual_left);
    table_links.sort();

    write_table(out, &entries, &table_links)?;
    eprintln!(
        "写好 {}：{} 条中文名（模组自带 {}，我们翻的 {}），{} 对关联",
        out.display(),
        entries.len(),
        entries.iter().filter(|e| e.from == NameFrom::Mod).count(),
        entries
            .iter()
            .filter(|e| e.from == NameFrom::Manual)
            .count(),
        table_links.len()
    );

    if let Some(report) = report {
        let mut text = String::from("名次\t总下载量\t英文名\t中文名\t来源\tModrinth\tCurseForge\n");
        let named: HashMap<(Option<&str>, Option<u32>), &NameEntry> = entries
            .iter()
            .map(|e| ((e.mr.as_deref(), e.cf), e))
            .collect();
        for (rank, p) in projects.iter().enumerate() {
            let entry = named.get(&(p.mr.as_deref(), p.cf));
            text.push_str(&format!(
                "{}\t{:.0}\t{}\t{}\t{}\t{}\t{}\n",
                rank + 1,
                p.downloads(),
                p.en,
                entry.map_or("", |e| e.zh.as_str()),
                entry.map_or("", |e| match e.from {
                    NameFrom::Mod => "模组自带",
                    NameFrom::Manual => "我们翻的",
                }),
                p.mr.as_deref().unwrap_or(""),
                p.cf.map(|n| n.to_string()).unwrap_or_default(),
            ));
        }
        write_atomic(report, text.as_bytes())?;
        eprintln!("核对表：{}", report.display());
    }
    Ok(())
}

/// 一条一行，改动好比对
fn write_table(out: &Path, entries: &[NameEntry], links: &[(String, u32)]) -> Result<()> {
    let mut text = Vec::new();
    writeln!(text, "{{")?;
    writeln!(text, "  \"format\": 1,")?;
    writeln!(text, "  \"license\": \"CC0-1.0\",")?;
    writeln!(text, "  \"entries\": [")?;
    for (i, entry) in entries.iter().enumerate() {
        let comma = if i + 1 < entries.len() { "," } else { "" };
        writeln!(text, "    {}{comma}", serde_json::to_string(entry)?)?;
    }
    writeln!(text, "  ],")?;
    writeln!(text, "  \"links\": [")?;
    for (i, link) in links.iter().enumerate() {
        let comma = if i + 1 < links.len() { "," } else { "" };
        writeln!(text, "    {}{comma}", serde_json::to_string(link)?)?;
    }
    writeln!(text, "  ]")?;
    writeln!(text, "}}")?;
    write_atomic(out, &text)?;
    Ok(())
}

/// `bytes 100-199/12345` → (100, 12345)
fn content_range(value: &str) -> Option<(u64, u64)> {
    let rest = value.strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.parse().ok()?, total.parse().ok()?))
}

/// 按需分段读远程文件（给压缩包库用的 Read + Seek）。读过的段记在内存里。
struct RangeReader<F> {
    fetch: F,
    len: u64,
    pos: u64,
    /// (开始位置, 内容)
    blocks: Vec<(u64, Vec<u8>)>,
}

impl<F: FnMut(String) -> std::io::Result<(u64, u64, Vec<u8>)>> RangeReader<F> {
    /// 先问文件多大，再读结尾一段（压缩包的目录在结尾）。
    /// CDN 不认「从结尾往前数」的写法（`bytes=-N` 返回 501），所以分两次。
    fn open(mut fetch: F) -> std::io::Result<Self> {
        let (_, len, head) = fetch("bytes=0-0".to_owned())?;
        if len == 0 {
            return Err(std::io::Error::other("文件是空的"));
        }
        let start = len.saturating_sub(BLOCK);
        let (got, _, tail) = fetch(format!("bytes={start}-{}", len - 1))?;
        if got != start {
            return Err(std::io::Error::other("分段请求返回的位置不对"));
        }
        Ok(Self {
            fetch,
            len,
            pos: 0,
            blocks: vec![(0, head), (start, tail)],
        })
    }
}

impl<F: FnMut(String) -> std::io::Result<(u64, u64, Vec<u8>)>> Read for RangeReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let pos = self.pos;
        let found = self
            .blocks
            .iter()
            .position(|(start, data)| pos >= *start && pos < start + data.len() as u64);
        let index = match found {
            Some(index) => index,
            None => {
                let start = pos / BLOCK * BLOCK;
                let want = BLOCK.max((buf.len() as u64).div_ceil(BLOCK) * BLOCK);
                let end = (start + want).min(self.len) - 1;
                let (got, _, data) = (self.fetch)(format!("bytes={start}-{end}"))?;
                if got != start || data.is_empty() {
                    return Err(std::io::Error::other("分段请求返回的位置不对"));
                }
                self.blocks.push((start, data));
                self.blocks.len() - 1
            }
        };
        let Some((start, data)) = self.blocks.get(index) else {
            return Ok(0);
        };
        let offset = usize::try_from(pos - start).map_err(std::io::Error::other)?;
        let n = (data.len() - offset).min(buf.len());
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<F> Seek for RangeReader<F> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
        };
        self.pos =
            u64::try_from(target).map_err(|_| std::io::Error::other("移到了文件开头之前"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_reader_reads_across_blocks() {
        let data: Vec<u8> = (0..(BLOCK * 3 + 17)).map(|i| (i % 251) as u8).collect();
        let source = data.clone();
        let fetch = |range: String| {
            let len = source.len() as u64;
            let spec = range.strip_prefix("bytes=").unwrap();
            let (start, end) = if let Some(n) = spec.strip_prefix('-') {
                (len - n.parse::<u64>().unwrap(), len - 1)
            } else {
                let (a, b) = spec.split_once('-').unwrap();
                (a.parse().unwrap(), b.parse::<u64>().unwrap())
            };
            Ok((start, len, source[start as usize..=end as usize].to_vec()))
        };
        let mut reader = RangeReader::open(fetch).unwrap();
        let mut all = Vec::new();
        reader.read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        reader.seek(SeekFrom::End(-5)).unwrap();
        let mut tail = [0u8; 5];
        reader.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, &data[data.len() - 5..]);
        assert_eq!(content_range("bytes 5-9/100"), Some((5, 100)));
    }
}
