//! CurseForge 的文件指纹：去掉空白字节（制表、换行、回车、空格）后算 MurmurHash2（种子 1）。
//!
//! 按公开的算法自己实现，用接口返回的真实指纹核对过。
//! 指纹要先知道「去掉空白后的长度」，所以大文件分两遍读：第一遍数长度，第二遍算哈希，不用整个读进内存。

use std::io::{self, BufReader, Read};
use std::path::Path;

const M: u32 = 0x5bd1_e995;
const SEED: u32 = 1;
const BUFFER: usize = 64 * 1024;

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 9 | 10 | 13 | 32)
}

/// 逐字节喂进去的 MurmurHash2（每凑满 4 个字节混合一次）。
struct Murmur2 {
    h: u32,
    tail: [u8; 4],
    tail_len: usize,
}

impl Murmur2 {
    fn new(len: u32) -> Self {
        Self {
            h: SEED ^ len,
            tail: [0; 4],
            tail_len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        self.tail[self.tail_len] = byte;
        self.tail_len += 1;
        if self.tail_len == 4 {
            let mut k = u32::from_le_bytes(self.tail);
            k = k.wrapping_mul(M);
            k ^= k >> 24;
            k = k.wrapping_mul(M);
            self.h = self.h.wrapping_mul(M) ^ k;
            self.tail_len = 0;
        }
    }

    fn finish(self) -> u32 {
        let mut h = self.h;
        let tail = self.tail;
        if self.tail_len >= 3 {
            h ^= u32::from(tail[2]) << 16;
        }
        if self.tail_len >= 2 {
            h ^= u32::from(tail[1]) << 8;
        }
        if self.tail_len >= 1 {
            h ^= u32::from(tail[0]);
            h = h.wrapping_mul(M);
        }
        h ^= h >> 13;
        h = h.wrapping_mul(M);
        h ^ (h >> 15)
    }
}

/// 一段内容的 CurseForge 指纹。
pub fn fingerprint(bytes: &[u8]) -> u32 {
    // 算法规定长度按 32 位算；超过 4 GB 的文件（不会出现在 CurseForge 上）按截断处理
    let len = bytes.iter().filter(|b| !is_whitespace(**b)).count() as u32;
    let mut hasher = Murmur2::new(len);
    for &byte in bytes.iter().filter(|b| !is_whitespace(**b)) {
        hasher.push(byte);
    }
    hasher.finish()
}

/// 文件的 CurseForge 指纹（阻塞读文件，在后台线程里调用）。
pub fn fingerprint_file(path: &Path) -> io::Result<u32> {
    let mut len: u64 = 0;
    for_each_chunk(path, |chunk| {
        len += chunk.iter().filter(|b| !is_whitespace(**b)).count() as u64;
    })?;
    let mut hasher = Murmur2::new(len as u32);
    for_each_chunk(path, |chunk| {
        for &byte in chunk.iter().filter(|b| !is_whitespace(**b)) {
            hasher.push(byte);
        }
    })?;
    Ok(hasher.finish())
}

fn for_each_chunk(path: &Path, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let mut reader = BufReader::with_capacity(BUFFER, std::fs::File::open(path)?);
    let mut buffer = vec![0u8; BUFFER];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        f(&buffer[..read]);
    }
}

/// [`fingerprint_file`] 的异步版本（放到后台线程）。
pub async fn fingerprint_file_async(path: &Path) -> io::Result<u32> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || fingerprint_file(&path))
        .await
        .map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这些值来自另一份独立实现（Python），那份实现算出的整合包指纹和 CurseForge 接口返回的一致
    #[test]
    fn matches_the_reference_implementation() {
        let all_bytes: Vec<u8> = (0..=255).collect();
        let cases: [(&[u8], u32); 8] = [
            (b"", 1_540_447_798),
            (b"a", 626_045_324),
            (b"ab", 1_692_487_918),
            (b"abc", 1_621_425_345),
            (b"abcd", 3_376_380_438),
            (b"hello world", 2_824_650_221),
            (b"Hello,\r\n\tWorld!  ", 1_961_219_979),
            (&all_bytes, 2_094_645_347),
        ];
        for (input, expected) in cases {
            assert_eq!(fingerprint(input), expected, "{input:?}");
        }
    }

    #[test]
    fn whitespace_does_not_change_the_fingerprint() {
        assert_eq!(fingerprint(b"a b\tc\r\nd"), fingerprint(b"abcd"));
    }

    #[test]
    fn file_version_matches_in_memory_version() {
        let file = tempfile::NamedTempFile::new().unwrap();
        // 比缓冲区大，跨多个分块；夹着空白字节
        let bytes: Vec<u8> = (0..BUFFER * 2 + 7).map(|i| (i % 251) as u8).collect();
        std::fs::write(file.path(), &bytes).unwrap();
        assert_eq!(fingerprint_file(file.path()).unwrap(), fingerprint(&bytes));
    }
}
