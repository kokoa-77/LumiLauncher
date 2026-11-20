//! Maven 坐标 → 库文件相对路径。
//!
//! 格式：`group:artifact:version[:classifier][@extension]`
//! 例：`net.fabricmc:fabric-loader:0.16.9` → `net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar`

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoord {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

impl MavenCoord {
    pub fn parse(coordinate: &str) -> Option<Self> {
        let (main, extension) = match coordinate.split_once('@') {
            Some((main, ext)) if !ext.is_empty() => (main, ext.to_owned()),
            Some(_) => return None,
            None => (coordinate, "jar".to_owned()),
        };
        let parts: Vec<&str> = main.split(':').collect();
        let (group, artifact, version, classifier) = match parts.as_slice() {
            [g, a, v] => (*g, *a, *v, None),
            [g, a, v, c] => (*g, *a, *v, Some((*c).to_owned())),
            _ => return None,
        };
        if group.is_empty() || artifact.is_empty() || version.is_empty() {
            return None;
        }
        Some(Self {
            group: group.to_owned(),
            artifact: artifact.to_owned(),
            version: version.to_owned(),
            classifier: classifier.filter(|c| !c.is_empty()),
            extension,
        })
    }

    /// 库文件在 `libraries/` 下的相对路径（用 `/` 分隔）。
    /// 注意：坐标来自外部 json，拼到磁盘路径前必须经过 `fs::safe_join`。
    pub fn path(&self) -> String {
        let classifier = self
            .classifier
            .as_ref()
            .map(|c| format!("-{c}"))
            .unwrap_or_default();
        format!(
            "{}/{}/{}/{}-{}{}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.artifact,
            self.version,
            classifier,
            self.extension
        )
    }

    /// 合并继承链时判断「是不是同一个库」用的键：不含版本号，含 classifier。
    pub fn identity(&self) -> String {
        format!(
            "{}:{}:{}",
            self.group,
            self.artifact,
            self.classifier.as_deref().unwrap_or("")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_coordinate() {
        let c = MavenCoord::parse("net.fabricmc:fabric-loader:0.16.9").unwrap();
        assert_eq!(
            c.path(),
            "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(c.identity(), "net.fabricmc:fabric-loader:");
    }

    #[test]
    fn classifier_and_extension() {
        let c = MavenCoord::parse("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap();
        assert_eq!(
            c.path(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );

        let c = MavenCoord::parse("net.neoforged:neoforge:21.1.77:installer@zip").unwrap();
        assert_eq!(
            c.path(),
            "net/neoforged/neoforge/21.1.77/neoforge-21.1.77-installer.zip"
        );

        let c = MavenCoord::parse("de.oceanlabs.mcp:mcp_config:1.21.1@zip").unwrap();
        assert_eq!(
            c.path(),
            "de/oceanlabs/mcp/mcp_config/1.21.1/mcp_config-1.21.1.zip"
        );
    }

    #[test]
    fn rejects_malformed() {
        for bad in ["", "a:b", "a:b:c:d:e", ":b:c", "a::c", "a:b:c@"] {
            assert!(MavenCoord::parse(bad).is_none(), "应该拒绝：{bad:?}");
        }
    }

    #[test]
    fn identity_ignores_version_but_keeps_classifier() {
        let a = MavenCoord::parse("org.ow2.asm:asm:9.6").unwrap();
        let b = MavenCoord::parse("org.ow2.asm:asm:9.7.1").unwrap();
        let natives = MavenCoord::parse("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap();
        let main = MavenCoord::parse("org.lwjgl:lwjgl:3.3.3").unwrap();
        assert_eq!(a.identity(), b.identity());
        assert_ne!(natives.identity(), main.identity());
    }
}
