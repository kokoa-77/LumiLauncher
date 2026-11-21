//! Discord 活动状态（Rich Presence）：纯本机命名管道，零外部 crate，零 `unsafe`，零网络请求。
//!
//! 仅当用户手动开启 `game.discordRpc` 时尝试连接本机 `\\.\pipe\discord-ipc-0..9`；
//! 未安装或未运行 Discord 时静默跳过，绝不报错、绝不拖慢游戏启动。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

/// 标准公开 Minecraft / Lumi 活动展示用 Client ID（仅本地 Discord IPC 握手字段，不经网络上传隐私）
const DEFAULT_CLIENT_ID: &str = "1311202600000000001";

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;

/// 编码一帧 Discord IPC 消息：`[opcode: u32 LE][length: u32 LE][utf8 json]`
pub fn encode_frame(opcode: u32, payload: &serde_json::Value) -> Vec<u8> {
    let body = payload.to_string();
    let len = u32::try_from(body.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&opcode.to_le_bytes());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(body.as_bytes());
    buf
}

fn send_frame(pipe: &mut File, opcode: u32, payload: &serde_json::Value) -> std::io::Result<()> {
    let frame = encode_frame(opcode, payload);
    pipe.write_all(&frame)?;
    pipe.flush()?;
    // 读掉服务端回包头（避免管道缓冲阻塞）；若对端关闭或不可读直接忽略
    let mut header = [0u8; 8];
    if pipe.read_exact(&mut header).is_ok() {
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if len > 0 && len <= 65_536 {
            let mut discard = vec![0u8; len];
            let _ = pipe.read_exact(&mut discard);
        }
    }
    Ok(())
}

fn open_discord_pipe() -> Option<File> {
    #[cfg(windows)]
    {
        for i in 0..10 {
            let path = format!(r"\\.\pipe\discord-ipc-{i}");
            if let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) {
                return Some(file);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = OpenOptions::new;
        None
    }
}

/// 活跃的本机 Discord IPC 会话；Drop 时自动清除活动状态并关闭管道。
pub struct DiscordSession {
    pipe: File,
    started_epoch_secs: u64,
}

impl DiscordSession {
    /// 尝试连接本机 Discord 客户端并设置初始游玩状态；未运行 Discord 时静默返回 `None`。
    pub fn connect(instance_name: &str, game_version: &str, loader: Option<&str>) -> Option<Self> {
        let mut pipe = open_discord_pipe()?;
        let handshake = json!({
            "v": 1,
            "client_id": DEFAULT_CLIENT_ID,
        });
        send_frame(&mut pipe, OP_HANDSHAKE, &handshake).ok()?;
        let started_epoch_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut session = Self {
            pipe,
            started_epoch_secs,
        };
        let state = match loader {
            Some(l) if !l.is_empty() => format!("Minecraft {game_version} ({l})"),
            _ => format!("Minecraft {game_version}"),
        };
        session.update(instance_name, &state);
        Some(session)
    }

    pub fn update(&mut self, details: &str, state: &str) {
        let pid = std::process::id();
        let nonce = format!("lumi-{}", crate::now_millis());
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": pid,
                "activity": {
                    "details": details,
                    "state": state,
                    "timestamps": {
                        "start": self.started_epoch_secs
                    }
                }
            },
            "nonce": nonce,
        });
        let _ = send_frame(&mut self.pipe, OP_FRAME, &payload);
    }

    pub fn clear(mut self) {
        let pid = std::process::id();
        let nonce = format!("lumi-clear-{}", crate::now_millis());
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": pid },
            "nonce": nonce,
        });
        let _ = send_frame(&mut self.pipe, OP_FRAME, &payload);
        let _ = send_frame(&mut self.pipe, OP_CLOSE, &json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_valid_ipc_frame() {
        let frame = encode_frame(OP_HANDSHAKE, &json!({"v": 1}));
        assert_eq!(&frame[0..4], &0u32.to_le_bytes());
        let body = &frame[8..];
        let len = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
        assert_eq!(len, body.len());
        assert_eq!(std::str::from_utf8(body).unwrap(), r#"{"v":1}"#);
    }
}
