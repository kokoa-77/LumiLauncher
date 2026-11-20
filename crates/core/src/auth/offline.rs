//! 从历史名称解析离线 UUID，用于查找旧存档内的玩家数据。

use md5::{Digest, Md5};
use uuid::Uuid;

/// 和官方服务端、其他启动器一致的离线 UUID：`OfflinePlayer:<名字>` 的 MD5，按 UUID 第 3 版设置标志位。
/// 同一个名字永远得到同一个 UUID，换启动器存档里的背包和进度不会丢。
pub fn offline_uuid(name: &str) -> Uuid {
    let digest = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest);
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // 版本 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // IETF 变体
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_well_known_offline_uuid() {
        // 社区里广泛引用的标准值：Notch 的离线 UUID
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn is_stable_and_version_3() {
        assert_eq!(offline_uuid("Steve"), offline_uuid("Steve"));
        assert_ne!(offline_uuid("Steve"), offline_uuid("steve"), "区分大小写");
        assert_eq!(offline_uuid("Steve").get_version_num(), 3);
    }
}
