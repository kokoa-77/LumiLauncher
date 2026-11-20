//! version json 里的 `rules` 判定。
//!
//! 规则：没有 rules → 允许；有 rules → 默认不允许，从上到下检查，
//! 每条匹配当前环境的规则都会改写结果，**最后一条匹配的规则说了算**。

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OsRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// 正则，匹配系统版本号
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// 判定规则时的运行环境。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// Mojang 的叫法：`windows` / `osx` / `linux`
    pub os_name: String,
    /// `x86_64` / `x86` / `arm64`
    pub os_arch: String,
    /// 系统版本号，Windows 10 和 11 都是 `10.0`（和 Java 报告的一致）
    pub os_version: String,
    /// 启动器开启的功能开关（例如 `has_custom_resolution`），没列出的都算关闭
    pub features: BTreeMap<String, bool>,
}

impl Environment {
    /// 当前电脑的环境。
    pub fn current() -> Self {
        let os_name = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "osx",
            _ => "linux",
        };
        let os_arch = match std::env::consts::ARCH {
            "x86" => "x86",
            "aarch64" => "arm64",
            other => other,
        };
        let os_version = if cfg!(windows) { "10.0" } else { "" };
        Self {
            os_name: os_name.to_owned(),
            os_arch: os_arch.to_owned(),
            os_version: os_version.to_owned(),
            features: BTreeMap::new(),
        }
    }

    pub fn with_feature(mut self, name: &str, enabled: bool) -> Self {
        self.features.insert(name.to_owned(), enabled);
        self
    }
}

impl Rule {
    pub fn matches(&self, env: &Environment) -> bool {
        if let Some(os) = &self.os {
            if os.name.as_deref().is_some_and(|name| name != env.os_name) {
                return false;
            }
            if os.arch.as_deref().is_some_and(|arch| arch != env.os_arch) {
                return false;
            }
            if let Some(pattern) = &os.version {
                // 规则里的正则写错了就当不匹配，不让一个坏规则导致崩溃
                let matched = Regex::new(pattern).is_ok_and(|re| re.is_match(&env.os_version));
                if !matched {
                    return false;
                }
            }
        }
        if let Some(features) = &self.features {
            for (name, wanted) in features {
                if env.features.get(name).copied().unwrap_or(false) != *wanted {
                    return false;
                }
            }
        }
        true
    }
}

/// 这组规则在当前环境下是否允许。
pub fn rules_allow(rules: Option<&[Rule]>, env: &Environment) -> bool {
    let Some(rules) = rules.filter(|r| !r.is_empty()) else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        if rule.matches(env) {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows() -> Environment {
        Environment {
            os_name: "windows".into(),
            os_arch: "x86_64".into(),
            os_version: "10.0".into(),
            features: BTreeMap::new(),
        }
    }

    fn parse(json: &str) -> Vec<Rule> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn no_rules_means_allowed() {
        assert!(rules_allow(None, &windows()));
        assert!(rules_allow(Some(&[]), &windows()));
    }

    #[test]
    fn os_only_allow() {
        let rules = parse(r#"[{"action":"allow","os":{"name":"windows"}}]"#);
        assert!(rules_allow(Some(&rules), &windows()));
        let mut linux = windows();
        linux.os_name = "linux".into();
        assert!(!rules_allow(Some(&rules), &linux));
    }

    #[test]
    fn last_matching_rule_wins() {
        // 老版本常见写法：允许全部，但排除 macOS
        let rules = parse(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]"#);
        assert!(rules_allow(Some(&rules), &windows()));
        let mut mac = windows();
        mac.os_name = "osx".into();
        assert!(!rules_allow(Some(&rules), &mac));
    }

    #[test]
    fn arch_and_version_regex() {
        let x86 = parse(r#"[{"action":"allow","os":{"arch":"x86"}}]"#);
        assert!(!rules_allow(Some(&x86), &windows()));

        let win10 = parse(r#"[{"action":"allow","os":{"name":"windows","version":"^10\\."}}]"#);
        assert!(rules_allow(Some(&win10), &windows()));

        let broken = parse(r#"[{"action":"allow","os":{"version":"(unclosed"}}]"#);
        assert!(!rules_allow(Some(&broken), &windows()));
    }

    #[test]
    fn features_must_match_exactly() {
        let rules = parse(r#"[{"action":"allow","features":{"has_custom_resolution":true}}]"#);
        assert!(!rules_allow(Some(&rules), &windows()));
        let env = windows().with_feature("has_custom_resolution", true);
        assert!(rules_allow(Some(&rules), &env));
    }
}
