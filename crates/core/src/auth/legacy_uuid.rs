//! 旧存档里按玩家名算出的 UUID。
//! 早期版本和别家启动器给没有正版档案的名字存统计时用的就是它；Lumi 只用它在旧存档里找游玩时长等本地统计，
//! 不会用它构造游戏身份或启动游戏（启动只认官方核验过的 Microsoft 账户档案）。

use md5::{Digest, Md5};
use uuid::Uuid;

/// `OfflinePlayer:<名字>` 的 MD5，按 UUID 第 3 版设置标志位，和服务端、其他启动器算出来的一样。
/// 同一个名字永远得到同一个 UUID，所以能对上旧存档 `stats/` 里的文件名。
pub fn legacy_name_uuid(name: &str) -> Uuid {
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
    fn matches_the_well_known_value() {
        // 社区里广泛引用的标准值：名字 Notch 算出来的 UUID
        assert_eq!(
            legacy_name_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn is_stable_and_version_3() {
        assert_eq!(legacy_name_uuid("Steve"), legacy_name_uuid("Steve"));
        assert_ne!(
            legacy_name_uuid("Steve"),
            legacy_name_uuid("steve"),
            "区分大小写"
        );
        assert_eq!(legacy_name_uuid("Steve").get_version_num(), 3);
    }
}
