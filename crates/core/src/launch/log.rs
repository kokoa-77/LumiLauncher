//! 游戏日志解析。
//!
//! 用官方日志配置启动时，游戏往控制台输出 log4j 的 XML 事件（一条日志跨多行）；
//! 没用日志配置的老版本输出普通文本 `[12:00:00] [Render thread/INFO]: ...`。两种都要支持。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_uppercase().as_str() {
            "TRACE" => LogLevel::Trace,
            "DEBUG" => LogLevel::Debug,
            "INFO" => LogLevel::Info,
            "WARN" | "WARNING" => LogLevel::Warn,
            "ERROR" => LogLevel::Error,
            "FATAL" => LogLevel::Fatal,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub level: LogLevel,
    pub thread: Option<String>,
    pub message: String,
}

/// 把一行原始输出转成文字：先按 UTF-8，失败再按 GBK（中文 Windows 的默认编码）。
pub fn decode_line(bytes: &[u8]) -> String {
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    match std::str::from_utf8(trimmed) {
        Ok(text) => text.to_owned(),
        Err(_) => encoding_rs::GBK.decode(trimmed).0.into_owned(),
    }
}

/// 逐行喂进来，凑够一条完整日志就吐出来。
#[derive(Debug, Default)]
pub struct LogParser {
    pending: Option<String>,
}

impl LogParser {
    pub fn push_line(&mut self, line: &str) -> Option<LogLine> {
        if let Some(buffer) = &mut self.pending {
            buffer.push('\n');
            buffer.push_str(line);
            if line.contains("</log4j:Event>") {
                let event = self.pending.take().unwrap_or_default();
                return Some(parse_xml_event(&event));
            }
            return None;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("<log4j:Event") {
            if trimmed.contains("</log4j:Event>") {
                return Some(parse_xml_event(trimmed));
            }
            self.pending = Some(trimmed.to_owned());
            return None;
        }
        if line.trim().is_empty() {
            return None;
        }
        Some(parse_plain_line(line))
    }
}

fn parse_xml_event(event: &str) -> LogLine {
    let header_end = event.find('>').unwrap_or(event.len());
    let header = &event[..header_end];
    let level = attribute(header, "level")
        .and_then(|l| LogLevel::parse(&l))
        .unwrap_or(LogLevel::Info);
    let thread = attribute(header, "thread");
    let mut message =
        cdata_between(event, "<log4j:Message>", "</log4j:Message>").unwrap_or_default();
    if let Some(throwable) = cdata_between(event, "<log4j:Throwable>", "</log4j:Throwable>") {
        message.push('\n');
        message.push_str(&throwable);
    }
    LogLine {
        level,
        thread,
        message,
    }
}

/// `[12:00:00] [Render thread/INFO]: 消息` 或任意普通文本
fn parse_plain_line(line: &str) -> LogLine {
    let parsed = (|| {
        let rest = line.strip_prefix('[')?;
        let (_, rest) = rest.split_once("] [")?;
        let (tag, message) = rest.split_once("]: ")?;
        let (thread, level) = tag.rsplit_once('/')?;
        Some(LogLine {
            level: LogLevel::parse(level)?,
            thread: Some(thread.to_owned()),
            message: message.to_owned(),
        })
    })();
    parsed.unwrap_or_else(|| LogLine {
        level: guess_level(line),
        thread: None,
        message: line.to_owned(),
    })
}

/// 没有格式的输出（例如 Java 自己报错）按关键字猜级别。
/// 行首就是级别词的（例如 log4j 自己的 `WARN StatusConsoleListener ...`）以行首为准。
fn guess_level(line: &str) -> LogLevel {
    if let Some(level) = line.split_whitespace().next().and_then(LogLevel::parse) {
        return level;
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("exception")
        || lower.contains("error")
        || line.trim_start().starts_with("at ")
    {
        LogLevel::Error
    } else if lower.contains("warn") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

fn attribute(header: &str, name: &str) -> Option<String> {
    let start = header.find(&format!(" {name}=\""))? + name.len() + 3;
    let end = header[start..].find('"')? + start;
    Some(unescape(&header[start..end]))
}

fn cdata_between(event: &str, open: &str, close: &str) -> Option<String> {
    let start = event.find(open)? + open.len();
    let end = event[start..].find(close)? + start;
    let inner = event[start..end].trim();
    let content = inner
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .map(|s| s.replace("]]]]><![CDATA[>", "]]>"))
        .unwrap_or_else(|| unescape(inner));
    Some(content)
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_line_xml_events() {
        let mut parser = LogParser::default();
        let lines = [
            r#"<log4j:Event logger="net.minecraft.client.Minecraft" timestamp="1" level="INFO" thread="Render thread">"#,
            "  <log4j:Message><![CDATA[Setting user: Steve]]></log4j:Message>",
            "</log4j:Event>",
        ];
        assert_eq!(parser.push_line(lines[0]), None);
        assert_eq!(parser.push_line(lines[1]), None);
        let line = parser.push_line(lines[2]).unwrap();
        assert_eq!(line.level, LogLevel::Info);
        assert_eq!(line.thread.as_deref(), Some("Render thread"));
        assert_eq!(line.message, "Setting user: Steve");
    }

    #[test]
    fn includes_throwables_and_unescapes() {
        let event = r#"<log4j:Event logger="x" level="ERROR" thread="Worker &amp; Co"><log4j:Message><![CDATA[boom]]></log4j:Message><log4j:Throwable><![CDATA[java.lang.RuntimeException: x
	at a.b(C.java:1)]]></log4j:Throwable></log4j:Event>"#;
        let line = parse_xml_event(event);
        assert_eq!(line.level, LogLevel::Error);
        assert_eq!(line.thread.as_deref(), Some("Worker & Co"));
        assert!(line.message.starts_with("boom\njava.lang.RuntimeException"));
    }

    #[test]
    fn parses_plain_text_lines() {
        let mut parser = LogParser::default();
        let line = parser
            .push_line("[12:00:01] [Server thread/WARN]: Can't keep up!")
            .unwrap();
        assert_eq!(line.level, LogLevel::Warn);
        assert_eq!(line.thread.as_deref(), Some("Server thread"));
        assert_eq!(line.message, "Can't keep up!");

        let raw = parser
            .push_line("Exception in thread \"main\" java.lang.NoClassDefFoundError")
            .unwrap();
        assert_eq!(raw.level, LogLevel::Error);

        // D4 实测：NeoForge 的 log4j 打的警告里带「Error」这个词，但行首写明了是 WARN
        let warn = parser
            .push_line("WARN StatusConsoleListener Error parsing URI D:\\mc\\client-1.12.xml")
            .unwrap();
        assert_eq!(warn.level, LogLevel::Warn);
        assert_eq!(parser.push_line("   "), None);
    }

    #[test]
    fn decodes_utf8_and_gbk() {
        assert_eq!(decode_line("你好\r\n".as_bytes()), "你好");
        let (gbk, _, _) = encoding_rs::GBK.encode("存档");
        assert_eq!(decode_line(&gbk), "存档");
    }
}
