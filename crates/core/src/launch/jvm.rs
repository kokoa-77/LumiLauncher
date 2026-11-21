//! 用户能调的启动参数：垃圾回收预设 + 自定义 JVM 参数、游戏参数。
//!
//! 自定义参数是用户在设置里输入的一行字，启动时按空白和引号拆成数组交给系统，
//! 不拼成一条命令字符串（docs/DESIGN.md 第 8 节第 6 条）。

use crate::config::GcPreset;

/// 官方启动器给 G1 的参数（所有 Java 版本都能用，Java 8 也有 G1）
const G1_ARGS: &[&str] = &[
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+UseG1GC",
    "-XX:G1NewSizePercent=20",
    "-XX:G1ReservePercent=20",
    "-XX:MaxGCPauseMillis=50",
    "-XX:G1HeapRegionSize=32M",
];

/// 这次启动要加的全部「额外 JVM 参数」：垃圾回收预设在前，用户自己写的在后。
/// 用户自己写了垃圾回收器（`-XX:+UseXxxGC`）时不加预设：同时指定两个，Java 会直接报错退出。
pub fn extra_jvm_args(preset: GcPreset, java_major: u32, custom: &str) -> Vec<String> {
    let custom = split_args(custom);
    let mut args = if custom.iter().any(|a| picks_collector(a)) {
        Vec::new()
    } else {
        gc_args(preset, java_major)
    };
    args.extend(custom);
    args
}

/// 垃圾回收预设对应的参数
pub fn gc_args(preset: GcPreset, java_major: u32) -> Vec<String> {
    let tokens: &[&str] = match preset {
        GcPreset::Off => &[],
        GcPreset::Auto => G1_ARGS,
        // ZGC：Java 15 起正式可用；21、22 的「分代」要手动打开，23 起默认就是分代
        // （24 起再写 ZGenerational 会警告），更老的 Java 没有 ZGC，退回 G1
        GcPreset::Zgc => match java_major {
            0..=14 => G1_ARGS,
            15..=20 => &["-XX:+UseZGC"],
            21 | 22 => &["-XX:+UseZGC", "-XX:+ZGenerational"],
            _ => &["-XX:+UseZGC"],
        },
    };
    tokens.iter().map(|&t| t.to_owned()).collect()
}

/// `-XX:+UseG1GC`、`-XX:+UseZGC`、`-XX:+UseShenandoahGC`……（选垃圾回收器的参数）
fn picks_collector(arg: &str) -> bool {
    arg.strip_prefix("-XX:+Use")
        .is_some_and(|rest| rest.ends_with("GC") && rest.len() > 2)
}

/// 把一行参数拆成数组：空白（包括全角空格）分隔，双引号或单引号包起来的部分里可以有空格。
/// 反斜杠不当转义符（Windows 路径里全是反斜杠）。
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_token = true;
            }
            None if c.is_whitespace() => {
                if in_token {
                    args.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            None => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_whitespace_and_respects_quotes() {
        assert_eq!(
            split_args("  -Xss2M   -Dfoo=bar\t--fullscreen "),
            ["-Xss2M", "-Dfoo=bar", "--fullscreen"]
        );
        assert_eq!(
            split_args(r#"-Dpath="C:\Program Files\x" '-Dname=a b'"#),
            [r"-Dpath=C:\Program Files\x", "-Dname=a b"]
        );
        assert_eq!(split_args("-Da　-Db"), ["-Da", "-Db"], "全角空格也算分隔");
        assert_eq!(
            split_args(r#"--title """#),
            ["--title", ""],
            "空引号是一个空参数"
        );
        assert!(split_args("   ").is_empty());
        assert_eq!(split_args(r#""没闭合 的引号"#), ["没闭合 的引号"]);
    }

    #[test]
    fn auto_preset_is_the_official_g1_flags() {
        let args = gc_args(GcPreset::Auto, 21);
        assert!(args.contains(&"-XX:+UseG1GC".to_owned()));
        assert_eq!(gc_args(GcPreset::Auto, 8), args, "Java 8 也用同一套");
        assert!(gc_args(GcPreset::Off, 21).is_empty());
    }

    #[test]
    fn zgc_follows_the_java_version() {
        assert_eq!(gc_args(GcPreset::Zgc, 8), gc_args(GcPreset::Auto, 8));
        assert_eq!(gc_args(GcPreset::Zgc, 17), ["-XX:+UseZGC"]);
        assert_eq!(
            gc_args(GcPreset::Zgc, 21),
            ["-XX:+UseZGC", "-XX:+ZGenerational"]
        );
        assert_eq!(gc_args(GcPreset::Zgc, 25), ["-XX:+UseZGC"]);
    }

    #[test]
    fn custom_collector_replaces_the_preset() {
        let args = extra_jvm_args(GcPreset::Auto, 21, "-XX:+UseShenandoahGC -Xss4M");
        assert_eq!(args, ["-XX:+UseShenandoahGC", "-Xss4M"]);
        // 只是调 G1 的细节、没换回收器：预设照加，用户的写在后面（后写的生效）
        let tuned = extra_jvm_args(GcPreset::Auto, 21, "-XX:MaxGCPauseMillis=30");
        assert_eq!(
            tuned.first().map(String::as_str),
            Some("-XX:+UnlockExperimentalVMOptions")
        );
        assert_eq!(
            tuned.last().map(String::as_str),
            Some("-XX:MaxGCPauseMillis=30")
        );
        assert!(extra_jvm_args(GcPreset::Off, 21, "").is_empty());
    }

    #[test]
    fn recognises_collector_flags_only() {
        assert!(picks_collector("-XX:+UseG1GC"));
        assert!(picks_collector("-XX:+UseParallelGC"));
        assert!(!picks_collector("-XX:+UseStringDeduplication"));
        assert!(!picks_collector("-XX:-UseG1GC"));
        assert!(!picks_collector("-XX:+UseGC"));
    }
}
