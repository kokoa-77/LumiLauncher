//! 世界笔记（D17j）：每个世界记几个坐标、待办和随手记，存在实例的 `Lumi/world-notes.json`，
//! 键是存档文件夹名。首页世界名下面显示第一条没做完的待办（没有待办就显示随手记的第一行）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::{AtomicWriteError, UnsafePath, read_json, safe_file_name, write_json_atomic};
use crate::instance::META_DIR;

#[derive(Debug, thiserror::Error)]
pub enum NotesError {
    #[error(transparent)]
    UnsafePath(#[from] UnsafePath),
    #[error("保存世界笔记失败")]
    Write(#[from] AtomicWriteError),
}

const NOTES_FILE: &str = "world-notes.json";
const MAX_PLACES: usize = 50;
const MAX_TODOS: usize = 100;
const MAX_NAME_CHARS: usize = 40;
const MAX_TODO_CHARS: usize = 200;
const MAX_TEXT_CHARS: usize = 5000;
/// 坐标的合理范围（世界边界是 ±3000 万，高度 -2048..2048 留够余量）
const MAX_COORD: i32 = 30_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum Dimension {
    #[default]
    Overworld,
    Nether,
    End,
}

/// 一个坐标（基地、矿洞、村庄……）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    #[serde(default)]
    pub dimension: Dimension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// 一个世界的笔记
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct WorldNotes {
    #[serde(default)]
    pub places: Vec<Place>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub text: String,
}

/// 首页世界名下面那一行
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase", tag = "kind", content = "text")]
pub enum NoteLine {
    /// 第一条没做完的待办
    Todo(String),
    /// 随手记的第一行
    Text(String),
}

impl WorldNotes {
    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.todos.is_empty() && self.text.trim().is_empty()
    }

    /// 首页显示哪一行：第一条没做完的待办，没有就是随手记的第一行
    pub fn line(&self) -> Option<NoteLine> {
        if let Some(todo) = self.todos.iter().find(|t| !t.done) {
            return Some(NoteLine::Todo(todo.text.clone()));
        }
        self.text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| NoteLine::Text(l.to_owned()))
    }

    /// 存之前整理：去掉空的、太长的截短、数量和坐标范围有上限
    fn cleaned(mut self) -> Self {
        let cut = |s: &str, max: usize| s.trim().chars().take(max).collect::<String>();
        self.places = self
            .places
            .into_iter()
            .take(MAX_PLACES)
            .map(|p| Place {
                name: cut(&p.name, MAX_NAME_CHARS),
                x: p.x.clamp(-MAX_COORD, MAX_COORD),
                y: p.y.clamp(-4096, 4096),
                z: p.z.clamp(-MAX_COORD, MAX_COORD),
                dimension: p.dimension,
            })
            .collect();
        self.todos = self
            .todos
            .into_iter()
            .map(|t| Todo {
                text: cut(&t.text, MAX_TODO_CHARS),
                done: t.done,
            })
            .filter(|t| !t.text.is_empty())
            .take(MAX_TODOS)
            .collect();
        self.text = self.text.trim_end().chars().take(MAX_TEXT_CHARS).collect();
        self
    }
}

type AllNotes = BTreeMap<String, WorldNotes>;

fn notes_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(META_DIR).join(NOTES_FILE)
}

/// 这个实例所有世界的笔记（读不了当作没有）
pub fn load_all(instance_dir: &Path) -> BTreeMap<String, WorldNotes> {
    match read_json(&notes_path(instance_dir)) {
        Ok(notes) => notes.unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, "世界笔记读不了，当作没有");
            AllNotes::new()
        }
    }
}

/// 一个世界的笔记
pub fn load(instance_dir: &Path, folder: &str) -> WorldNotes {
    load_all(instance_dir).remove(folder).unwrap_or_default()
}

/// 存一个世界的笔记（整理后的样子），全空就删掉这一条。返回存下的样子
pub fn save(
    instance_dir: &Path,
    folder: &str,
    notes: WorldNotes,
) -> Result<WorldNotes, NotesError> {
    let folder = safe_file_name(folder)?;
    let notes = notes.cleaned();
    let mut all = load_all(instance_dir);
    if notes.is_empty() {
        all.remove(folder);
    } else {
        all.insert(folder.to_owned(), notes.clone());
    }
    write_json_atomic(&notes_path(instance_dir), &all)?;
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_cleans_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let notes = WorldNotes {
            places: vec![Place {
                name: "  家  ".to_owned(),
                x: 120,
                y: 64,
                z: -340,
                dimension: Dimension::Overworld,
            }],
            todos: vec![
                Todo {
                    text: "砍树".to_owned(),
                    done: true,
                },
                Todo {
                    text: "   ".to_owned(),
                    done: false,
                },
                Todo {
                    text: "去找要塞".to_owned(),
                    done: false,
                },
            ],
            text: "\n种子是朋友给的\n".to_owned(),
        };
        let saved = save(dir.path(), "新的世界", notes).unwrap();
        assert_eq!(saved.places[0].name, "家");
        assert_eq!(saved.todos.len(), 2, "空的待办去掉");
        assert_eq!(load(dir.path(), "新的世界"), saved);
        assert_eq!(saved.line(), Some(NoteLine::Todo("去找要塞".to_owned())));

        // 待办都做完了：显示随手记第一行
        let mut done = saved.clone();
        done.todos.iter_mut().for_each(|t| t.done = true);
        assert_eq!(
            done.line(),
            Some(NoteLine::Text("种子是朋友给的".to_owned()))
        );

        // 清空就删掉这一条
        save(dir.path(), "新的世界", WorldNotes::default()).unwrap();
        assert!(load_all(dir.path()).is_empty());
        assert!(save(dir.path(), "../evil", WorldNotes::default()).is_err());
    }
}
