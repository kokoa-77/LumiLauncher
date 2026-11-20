//! 最小的 NBT 读写器：够读 `level.dat`、读写 `servers.dat` 就行（按公开格式自己实现）。
//!
//! 格式：大端；每个带名字的标签是「类型 1 字节 + 名字（2 字节长度 + 修改版 UTF-8）+ 内容」。
//! 数组类（字节数组、整数数组、长整数数组）只记长度、不留内容，`level.dat` 里用不到它们。
//! 存档可能被改坏，所以每次读之前都检查剩余长度，嵌套最多 64 层。

use std::collections::BTreeMap;

const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(Vec<Value>),
    Compound(BTreeMap<String, Value>),
    /// 字节数组、整数数组、长整数数组：只记元素个数
    Array(usize),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Compound(map) => map.get(key),
            _ => None,
        }
    }

    /// 整数类的值（字节、短整数、整数、长整数都算）
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Byte(v) => Some(i64::from(v)),
            Value::Short(v) => Some(i64::from(v)),
            Value::Int(v) => Some(i64::from(v)),
            Value::Long(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NbtError {
    #[error("数据提前结束")]
    Truncated,
    #[error("不认识的标签类型 {0}")]
    UnknownTag(u8),
    #[error("嵌套太深")]
    TooDeep,
    #[error("根标签不是复合标签")]
    NotCompound,
    #[error("这份数据里有数组，读的时候没保留内容，不能原样写回")]
    ArrayNotKept,
    #[error("字符串太长")]
    TooLong,
}

/// 读整份 NBT（已经解压过的字节），返回根复合标签。
pub fn parse(bytes: &[u8]) -> Result<Value, NbtError> {
    let mut reader = Reader { bytes, pos: 0 };
    let tag = reader.u8()?;
    if tag != 10 {
        return Err(NbtError::NotCompound);
    }
    reader.string()?; // 根标签的名字（通常为空）
    reader.payload(tag, 0)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], NbtError> {
        let end = self.pos.checked_add(n).ok_or(NbtError::Truncated)?;
        let slice = self.bytes.get(self.pos..end).ok_or(NbtError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NbtError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }

    /// 长度字段：负数按 0 处理；比剩下的字节还多时直接判定数据不完整（防止按错误长度分配内存）
    fn length(&mut self, element_size: usize) -> Result<usize, NbtError> {
        let len = usize::try_from(i32::from_be_bytes(self.array()?)).unwrap_or(0);
        let remaining = self.bytes.len() - self.pos;
        if len.saturating_mul(element_size.max(1)) > remaining {
            return Err(NbtError::Truncated);
        }
        Ok(len)
    }

    fn string(&mut self) -> Result<String, NbtError> {
        let len = usize::from(u16::from_be_bytes(self.array()?));
        Ok(decode_modified_utf8(self.take(len)?))
    }

    fn payload(&mut self, tag: u8, depth: usize) -> Result<Value, NbtError> {
        if depth > MAX_DEPTH {
            return Err(NbtError::TooDeep);
        }
        Ok(match tag {
            1 => Value::Byte(i8::from_be_bytes(self.array()?)),
            2 => Value::Short(i16::from_be_bytes(self.array()?)),
            3 => Value::Int(i32::from_be_bytes(self.array()?)),
            4 => Value::Long(i64::from_be_bytes(self.array()?)),
            5 => Value::Float(f32::from_be_bytes(self.array()?)),
            6 => Value::Double(f64::from_be_bytes(self.array()?)),
            7 | 11 | 12 => {
                let size = match tag {
                    7 => 1,
                    11 => 4,
                    _ => 8,
                };
                let len = self.length(size)?;
                self.take(len * size)?;
                Value::Array(len)
            }
            8 => Value::String(self.string()?),
            9 => {
                let item = self.u8()?;
                let len = self.length(1)?;
                if item == 0 && len > 0 {
                    return Err(NbtError::UnknownTag(0));
                }
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.payload(item, depth + 1)?);
                }
                Value::List(items)
            }
            10 => {
                let mut map = BTreeMap::new();
                loop {
                    let child = self.u8()?;
                    if child == 0 {
                        break;
                    }
                    let name = self.string()?;
                    let value = self.payload(child, depth + 1)?;
                    map.insert(name, value);
                }
                Value::Compound(map)
            }
            other => return Err(NbtError::UnknownTag(other)),
        })
    }
}

/// Java 的「修改版 UTF-8」：空字符写成两个字节，超出基本平面的字符写成两个三字节的代理项。
/// 先拼回 UTF-16 再转成字符串；格式不对的部分用替换字符代替，不报错。
fn decode_modified_utf8(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.contains('\u{0}')
    {
        // 绝大多数名字（含中文）和标准 UTF-8 完全一样，直接用
        return text.to_owned();
    }
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let (unit, width) = if b < 0x80 {
            (u16::from(b), 1)
        } else if b & 0xE0 == 0xC0 && i + 1 < bytes.len() {
            (
                (u16::from(b & 0x1F) << 6) | u16::from(bytes[i + 1] & 0x3F),
                2,
            )
        } else if b & 0xF0 == 0xE0 && i + 2 < bytes.len() {
            (
                (u16::from(b & 0x0F) << 12)
                    | (u16::from(bytes[i + 1] & 0x3F) << 6)
                    | u16::from(bytes[i + 2] & 0x3F),
                3,
            )
        } else {
            (0xFFFD, 1)
        };
        units.push(unit);
        i += width;
    }
    String::from_utf16_lossy(&units)
}

// ---------- 写 ----------

/// 把根复合标签写成 NBT 字节（不压缩；`servers.dat` 就是这种）。
/// 读的时候数组只记了长度，遇到就报错，免得把用户的文件写坏。
pub fn write(root: &Value) -> Result<Vec<u8>, NbtError> {
    let Value::Compound(_) = root else {
        return Err(NbtError::NotCompound);
    };
    let mut out = Vec::new();
    out.push(10);
    write_string(&mut out, "")?;
    write_payload(&mut out, root, 0)?;
    Ok(out)
}

fn tag_of(value: &Value) -> Result<u8, NbtError> {
    Ok(match value {
        Value::Byte(_) => 1,
        Value::Short(_) => 2,
        Value::Int(_) => 3,
        Value::Long(_) => 4,
        Value::Float(_) => 5,
        Value::Double(_) => 6,
        Value::String(_) => 8,
        Value::List(_) => 9,
        Value::Compound(_) => 10,
        Value::Array(_) => return Err(NbtError::ArrayNotKept),
    })
}

fn write_payload(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), NbtError> {
    if depth > MAX_DEPTH {
        return Err(NbtError::TooDeep);
    }
    match value {
        Value::Byte(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Value::String(v) => write_string(out, v)?,
        Value::List(items) => {
            // 空列表的元素类型写 0（结束标签），和游戏自己写的一样
            let element = match items.first() {
                Some(first) => tag_of(first)?,
                None => 0,
            };
            out.push(element);
            let len = i32::try_from(items.len()).map_err(|_| NbtError::TooLong)?;
            out.extend_from_slice(&len.to_be_bytes());
            for item in items {
                if tag_of(item)? != element {
                    // 列表里的元素类型必须一样，不一样就是数据本身不对
                    return Err(NbtError::UnknownTag(tag_of(item)?));
                }
                write_payload(out, item, depth + 1)?;
            }
        }
        Value::Compound(map) => {
            for (name, child) in map {
                out.push(tag_of(child)?);
                write_string(out, name)?;
                write_payload(out, child, depth + 1)?;
            }
            out.push(0);
        }
        Value::Array(_) => return Err(NbtError::ArrayNotKept),
    }
    Ok(())
}

fn write_string(out: &mut Vec<u8>, text: &str) -> Result<(), NbtError> {
    let bytes = encode_modified_utf8(text);
    let len = u16::try_from(bytes.len()).map_err(|_| NbtError::TooLong)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&bytes);
    Ok(())
}

/// Java 的「修改版 UTF-8」：空字符写两个字节，超出基本平面的字符拆成代理项、各写三个字节。
fn encode_modified_utf8(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x0001..=0x007F => out.push(unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                out.push(0xC0 | ((unit >> 6) as u8 & 0x1F));
                out.push(0x80 | (unit as u8 & 0x3F));
            }
            _ => {
                out.push(0xE0 | ((unit >> 12) as u8 & 0x0F));
                out.push(0x80 | ((unit >> 6) as u8 & 0x3F));
                out.push(0x80 | (unit as u8 & 0x3F));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 拼 NBT 用的小工具（测试里造 level.dat）
    pub(crate) struct Writer(pub Vec<u8>);

    impl Writer {
        pub fn new() -> Self {
            let mut w = Self(Vec::new());
            w.0.push(10);
            w.name("");
            w
        }
        fn name(&mut self, name: &str) {
            self.0
                .extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
            self.0.extend_from_slice(name.as_bytes());
        }
        pub fn begin(&mut self, name: &str) -> &mut Self {
            self.0.push(10);
            self.name(name);
            self
        }
        pub fn end(&mut self) -> &mut Self {
            self.0.push(0);
            self
        }
        pub fn string(&mut self, name: &str, value: &str) -> &mut Self {
            self.0.push(8);
            self.name(name);
            self.name(value);
            self
        }
        pub fn long(&mut self, name: &str, value: i64) -> &mut Self {
            self.0.push(4);
            self.name(name);
            self.0.extend_from_slice(&value.to_be_bytes());
            self
        }
        pub fn int(&mut self, name: &str, value: i32) -> &mut Self {
            self.0.push(3);
            self.name(name);
            self.0.extend_from_slice(&value.to_be_bytes());
            self
        }
        pub fn byte(&mut self, name: &str, value: i8) -> &mut Self {
            self.0.push(1);
            self.name(name);
            self.0.push(value.to_be_bytes()[0]);
            self
        }
        pub fn int_array(&mut self, name: &str, values: &[i32]) -> &mut Self {
            self.0.push(11);
            self.name(name);
            self.0
                .extend_from_slice(&i32::try_from(values.len()).unwrap().to_be_bytes());
            for v in values {
                self.0.extend_from_slice(&v.to_be_bytes());
            }
            self
        }
        pub fn finish(mut self) -> Vec<u8> {
            self.0.push(0);
            self.0
        }
    }

    #[test]
    fn writes_what_it_reads_back() {
        let mut server = BTreeMap::new();
        server.insert("name".to_owned(), Value::String("宝可梦服 🎮".to_owned()));
        server.insert(
            "ip".to_owned(),
            Value::String("play.example.com".to_owned()),
        );
        server.insert("hidden".to_owned(), Value::Byte(0));
        let mut root = BTreeMap::new();
        root.insert(
            "servers".to_owned(),
            Value::List(vec![Value::Compound(server)]),
        );
        root.insert("empty".to_owned(), Value::List(Vec::new()));
        root.insert("n".to_owned(), Value::Long(-7));
        root.insert("nul".to_owned(), Value::String("a\u{0}b".to_owned()));
        let root = Value::Compound(root);
        let bytes = write(&root).unwrap();
        assert_eq!(parse(&bytes).unwrap(), root);
    }

    #[test]
    fn refuses_to_write_arrays_it_did_not_keep() {
        let mut root = BTreeMap::new();
        root.insert("data".to_owned(), Value::Array(3));
        assert_eq!(write(&Value::Compound(root)), Err(NbtError::ArrayNotKept));
        assert_eq!(write(&Value::Int(1)), Err(NbtError::NotCompound));
    }

    #[test]
    fn reads_nested_compounds_and_skips_arrays() {
        let mut w = Writer::new();
        w.begin("Data")
            .string("LevelName", "新的世界")
            .long("DayTime", 2_400_000)
            .int_array("UUID", &[1, 2, 3, 4])
            .byte("hardcore", 1)
            .end();
        let root = parse(&w.finish()).unwrap();
        let data = root.get("Data").unwrap();
        assert_eq!(
            data.get("LevelName").and_then(Value::as_str),
            Some("新的世界")
        );
        assert_eq!(data.get("DayTime").and_then(Value::as_i64), Some(2_400_000));
        assert_eq!(data.get("hardcore").and_then(Value::as_i64), Some(1));
        assert_eq!(data.get("UUID"), Some(&Value::Array(4)));
    }

    #[test]
    fn rejects_broken_data_without_panicking() {
        let mut w = Writer::new();
        w.string("LevelName", "x");
        let mut bytes = w.finish();
        bytes.truncate(bytes.len() - 3);
        assert_eq!(parse(&bytes), Err(NbtError::Truncated));
        assert_eq!(parse(&[8, 0, 0]), Err(NbtError::NotCompound));
        // 数组长度写成 20 亿：直接判定不完整，不会去分配内存
        let huge = [10, 0, 0, 11, 0, 1, b'a', 0x7F, 0xFF, 0xFF, 0xFF];
        assert_eq!(parse(&huge), Err(NbtError::Truncated));
    }

    #[test]
    fn decodes_java_modified_utf8() {
        assert_eq!(decode_modified_utf8("世界 A".as_bytes()), "世界 A");
        // 空字符写成 C0 80
        assert_eq!(decode_modified_utf8(&[b'a', 0xC0, 0x80, b'b']), "a\u{0}b");
        // 😀（U+1F600）写成两个代理项：ED A0 BD ED B8 80
        assert_eq!(
            decode_modified_utf8(&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]),
            "😀"
        );
    }
}
