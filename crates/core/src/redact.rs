//! 日志脱敏：日志会被用户贴到 QQ 群、贴吧、GitHub，一次泄漏就是一个账号。
//! 所有写进日志和诊断包的文本都要经过 [`redact`]；token 类的值用 [`Secret`] 包起来，
//! 即使被 `{:?}` 打印也只会显示 `<redacted>`。

use std::borrow::Cow;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

pub const MASK: &str = "<redacted>";

struct Rule {
    pattern: Regex,
    replacement: &'static str,
}

#[allow(clippy::expect_used)] // 规则是写死的常量，单元测试覆盖了每一条，编译失败只可能是开发时写错
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, replacement: &'static str| Rule {
        pattern: Regex::new(pattern).expect("脱敏规则必须是合法正则"),
        replacement,
    };
    vec![
        // JSON 字段："accessToken": "xxx"
        rule(
            r#"(?i)("(?:access_?token|refresh_?token|id_?token|client_?secret|password|api[-_]?key|x-api-key|authorization|xuid)"\s*:\s*")(?:\\.|[^"\\])*(")"#,
            "${1}<redacted>${2}",
        ),
        // 查询参数或表单：access_token=xxx
        rule(
            r"(?i)\b(access_?token|refresh_?token|id_?token|client_?secret|password|api[-_]?key)=([^&\s]+)",
            "${1}=<redacted>",
        ),
        // 请求头：Authorization: Bearer xxx / x-api-key: xxx
        rule(
            r"(?i)\b(authorization[^\S\r\n]*:[^\S\r\n]*)[^\r\n]+",
            "${1}<redacted>",
        ),
        rule(r"(?i)\b(x-api-key\s*:\s*)\S+", "${1}<redacted>"),
        rule(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]+", "${1}<redacted>"),
        // Minecraft 启动参数：--accessToken xxx
        rule(
            r#"(?i)(--(?:access_?token|xuid|client_?secret)["']?(?:\s*[,=]\s*|\s+))(?:"(?:\\.|[^"\\])*"|'[^']*'|[^,\s\]]+)"#,
            "${1}<redacted>",
        ),
        // 任意位置的 JWT（三段 base64url）
        rule(
            r"\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}",
            "<redacted-jwt>",
        ),
    ]
});

/// 把文本里的敏感值替换成 `<redacted>`。没有命中时不分配新字符串。
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut current: Cow<'_, str> = Cow::Borrowed(text);
    for rule in RULES.iter() {
        if rule.pattern.is_match(&current) {
            current = Cow::Owned(
                rule.pattern
                    .replace_all(&current, rule.replacement)
                    .into_owned(),
            );
        }
    }
    current
}

/// 敏感值的包装。`Debug` 和 `Display` 都只显示 `<redacted>`，真实值必须显式调用 `expose`。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MASK)
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(MASK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_json_token_fields() {
        let input = r#"{"accessToken": "abc123", "refresh_token":"r-456", "name": "Steve"}"#;
        let out = redact(input);
        assert!(!out.contains("abc123") && !out.contains("r-456"));
        assert!(out.contains(r#""name": "Steve""#));
    }

    #[test]
    fn masks_query_parameters_and_headers() {
        let out = redact(
            "GET /x?access_token=SECRET&page=2 Authorization: Bearer SECRET2 x-api-key: K3Y",
        );
        for leaked in ["SECRET", "SECRET2", "K3Y"] {
            assert!(!out.contains(leaked), "泄漏了 {leaked}：{out}");
        }
        assert!(out.contains("page=2"));
    }

    #[test]
    fn masks_minecraft_access_token_argument() {
        let out = redact(
            "java -cp x net.minecraft.client.main.Main --username Steve --accessToken eyXYZ --version 1.21.1",
        );
        assert!(!out.contains("eyXYZ"));
        assert!(out.contains("--username Steve"));
        assert!(out.contains("--version 1.21.1"));
    }

    #[test]
    fn masks_bare_jwts() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let text = format!("token was {jwt} ok");
        let out = redact(&text);
        assert!(!out.contains(jwt));
        assert!(out.contains("<redacted-jwt>"));
    }

    #[test]
    fn leaves_ordinary_text_untouched_without_allocating() {
        let text = "正在下载 NeoForge 21.1.77，共 132 个文件";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn secret_never_prints_its_value() {
        let secret = Secret::new("super-secret".to_owned());
        assert_eq!(format!("{secret:?} {secret}"), "<redacted> <redacted>");
        assert_eq!(secret.expose(), "super-secret");
    }
}
