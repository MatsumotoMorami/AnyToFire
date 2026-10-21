use base64::Engine;
use serde_json::{Map, Value};
use std::ffi::{c_char, c_void, CString};

type Traversal = unsafe extern "system" fn(*mut c_void, i32) -> *mut c_void;
type Name = unsafe extern "system" fn(*mut c_void, *mut c_char, u32) -> i32;
type NodeType = unsafe extern "system" fn(*mut c_void) -> i32;
type DataSize = unsafe extern "system" fn(*mut c_void) -> i32;
type IsArray = unsafe extern "system" fn(*mut c_void) -> bool;
type Read = unsafe extern "system" fn(*mut c_void, i32, *mut c_void, u32) -> i32;

#[derive(Clone, Copy)]
pub struct AvsNodes { pub traversal: Traversal, pub name: Name, pub node_type: NodeType,
    pub data_size: DataSize, pub is_array: IsArray, pub read: Read }

#[link(name = "kernel32")]
extern "system" {
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn MultiByteToWideChar(codepage: u32, flags: u32, input: *const u8, input_len: i32, output: *mut u16, output_len: i32) -> i32;
}

fn shift_jis(bytes: &[u8]) -> Result<String, String> {
    let bytes=bytes.strip_suffix(&[0]).unwrap_or(bytes);
    if bytes.is_empty() { return Ok(String::new()); }
    let count=unsafe { MultiByteToWideChar(932, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0) };
    if count <= 0 { return Err("invalid Shift-JIS string".into()); }
    let mut wide=vec![0u16;count as usize];
    if unsafe { MultiByteToWideChar(932, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), count) } != count { return Err("Shift-JIS conversion failed".into()); }
    String::from_utf16(&wide).map_err(|_| "invalid UTF-16 conversion".into())
}

fn base_kind(kind: i32) -> i32 { kind & 0x3f }
fn is_element_kind(kind: i32) -> bool { matches!(base_kind(kind), 1 | 47) }

impl AvsNodes {
    pub unsafe fn resolve(module: *mut c_void) -> Option<Self> {
        unsafe fn get(module: *mut c_void, name: &str) -> Option<*mut c_void> {
            let name = CString::new(name).ok()?;
            let ptr = GetProcAddress(module, name.as_ptr());
            (!ptr.is_null()).then_some(ptr)
        }
        Some(Self { traversal: std::mem::transmute(get(module, "XCgsqzn00000a6")?),
            name: std::mem::transmute(get(module, "XCgsqzn00000a7")?),
            node_type: std::mem::transmute(get(module, "XCgsqzn00000a8")?),
            data_size: std::mem::transmute(get(module, "XCgsqzn00000aa")?),
            is_array: std::mem::transmute(get(module, "XCgsqzn00000a9")?),
            read: std::mem::transmute(get(module, "XCgsqzn00000ab")?) })
    }
    unsafe fn node_name(&self, node: *mut c_void) -> Result<String, String> {
        let mut name = [0u8; 256];
        if (self.name)(node, name.as_mut_ptr().cast(), name.len() as u32) < 0 { return Err("AVS node name failed".into()); }
        let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
        String::from_utf8(name[..end].to_vec()).map_err(|_| "AVS node name is not ASCII".into())
    }
    unsafe fn bytes(&self, node: *mut c_void, kind: i32) -> Result<Vec<u8>, String> {
        let size = (self.data_size)(node);
        if !(0..=262144).contains(&size) { return Err("AVS node size exceeds limit".into()); }
        // A present, empty binary/string/array node is valid. AVS read may
        // reject a zero-byte payload, so there is nothing to fetch in this case.
        if size == 0 { return Ok(Vec::new()); }
        let mut data = vec![0u8; size as usize + 1];
        let result = (self.read)(node, kind, data.as_mut_ptr().cast(), data.len() as u32);
        if result < 0 {
            let name = self.node_name(node).unwrap_or_else(|_| "?".into());
            return Err(format!("AVS node read failed: {name}, type {kind}, size {size}, code {result}"));
        }
        data.truncate(size as usize);
        Ok(data)
    }
    unsafe fn value(&self, node: *mut c_void, kind: i32, array: bool) -> Result<(String, Value), String> {
        let data = self.bytes(node, kind)?;
        if kind == 10 { return Ok(("BINARY".into(), Value::String(base64::engine::general_purpose::STANDARD.encode(data)))); }
        if kind == 11 || kind == 46 { return Ok(("STRING".into(), Value::String(shift_jis(&data)?))); }
        if kind == 12 {
            if data.len() < 4 { return Err("IP4 length invalid".into()); }
            return Ok(("IP4".into(), Value::String(format!("{}.{}.{}.{}",data[0],data[1],data[2],data[3]))));
        }
        let (label, width, signed) = match kind {
            2 => ("S8",1,true), 3 => ("U8",1,false), 4 => ("S16",2,true), 5 => ("U16",2,false),
            6 => ("S32",4,true), 7 => ("U32",4,false), 8 => ("S64",8,true), 9 => ("U64",8,false),
            13 => ("TIME",4,true), 52 | 56 => ("BOOLEAN",1,false),
            48 => ("S8",1,true), 49 => ("U8",1,false), 50 => ("S16",2,true), 51 => ("U16",2,false),
            _ => return Err(format!("unsupported AVS type {kind}")) };
        if data.len() % width != 0 || (!array && data.len() < width) { return Err("AVS scalar width invalid".into()); }
        let values: Vec<Value> = data.chunks_exact(width).map(|chunk| {
            let mut raw = [0u8;8];raw[..width].copy_from_slice(chunk);
            if label == "BOOLEAN" { return Value::Bool(raw[0] != 0); }
            if width == 8 { return Value::String(if signed { i64::from_le_bytes(raw).to_string() } else { u64::from_le_bytes(raw).to_string() }); }
            let value = u64::from_le_bytes(raw);
            if signed {
                let bits = width * 8;
                Value::from(((value << (64-bits)) as i64) >> (64-bits))
            } else { Value::from(value) }
        }).collect();
        if array || matches!(kind, 48..=51 | 56) { Ok((format!("{label}_ARRAY"), Value::Array(values))) }
        else { Ok((label.into(), values.into_iter().next().ok_or("empty scalar")?)) }
    }
    pub unsafe fn encode(&self, node: *mut c_void, depth: usize, count: &mut usize) -> Result<Value, String> {
        if node.is_null() || depth > 32 || *count >= 4096 { return Err("AVS tree limit".into()); }
        *count += 1;
        let name = self.node_name(node)?;
        let kind = (self.node_type)(node);
        let mut attrs = Map::new();
        let mut attr = (self.traversal)(node, 2);
        while !attr.is_null() {
            let key = self.node_name(attr)?;
            let (_, val) = self.value(attr, 46, false)?;
            attrs.insert(key, Value::String(val.as_str().ok_or("attribute is not string")?.to_owned()));
            attr = (self.traversal)(attr, 4);
        }
        let mut children = Vec::new();
        let mut child = (self.traversal)(node, 1);
        while !child.is_null() {
            children.push(self.encode(child, depth+1, count)?);
            child = (self.traversal)(child, 4);
        }
        let mut object = Map::new();
        object.insert("name".into(), Value::String(name));
        object.insert("attributes".into(), Value::Object(attrs));
        object.insert("children".into(), Value::Array(children));
        // AVS adds high flag bits to the base type: omission_log is 65 and
        // chattering_log is 193, both element nodes without scalar data.
        if is_element_kind(kind) { object.insert("type".into(), Value::String("ELEMENT".into())); }
        else { let (kind, value) = self.value(node, base_kind(kind), (self.is_array)(node))?;
            object.insert("type".into(), Value::String(kind)); object.insert("value".into(), value); }
        Ok(Value::Object(object))
    }
    pub unsafe fn encode_call(&self, node: *mut c_void) -> Result<Value, String> {
        let mut root = node;
        for _ in 0..3 {
            if self.node_name(root)? == "call" { break; }
            root = (self.traversal)(root, 0);
            if root.is_null() { return Err("music.reg call root unavailable".into()); }
        }
        if self.node_name(root)? != "call" { return Err("music.reg call root unavailable".into()); }
        let value = self.encode(root, 1, &mut 0)?;
        let children = value["children"].as_array().ok_or("invalid call children")?;
        if children.len() != 1 || !matches!(children[0]["name"].as_str(), Some("IIDX33music" | "IIDX34music")) || children[0]["attributes"]["method"] != "reg" { return Err("not a music.reg request".into()); }
        Ok(value)
    }
    pub unsafe fn reg_ack(&self, node: *mut c_void) -> Result<Option<(u32, u8)>, String> {
        if node.is_null() || !matches!(self.node_name(node)?.as_str(), "IIDX33music" | "IIDX34music") { return Ok(None); }
        let mut mid = None;
        let mut clid = None;
        let mut attr = (self.traversal)(node, 2);
        while !attr.is_null() {
            let key = self.node_name(attr)?;
            if matches!(key.as_str(), "mid" | "clid" | "status") {
                let (_, value) = self.value(attr, 46, false)?;
                let raw = value.as_str().ok_or("response attribute is not a string")?;
                match key.as_str() {
                    "mid" => mid = raw.parse::<u32>().ok(),
                    "clid" => clid = raw.parse::<u8>().ok(),
                    "status" if raw != "0" => return Ok(None),
                    _ => {}
                }
            }
            attr = (self.traversal)(attr, 4);
        }
        Ok(mid.zip(clid).filter(|(_, chart)| *chart < 10))
    }
}

#[cfg(test)]
mod tests {
    use super::{is_element_kind, AvsNodes};
    use std::ffi::{c_char, c_void};

    unsafe extern "system" fn traversal(_: *mut c_void, _: i32) -> *mut c_void { std::ptr::null_mut() }
    unsafe extern "system" fn name(_: *mut c_void, _: *mut c_char, _: u32) -> i32 { 0 }
    unsafe extern "system" fn node_type(_: *mut c_void) -> i32 { 10 }
    unsafe extern "system" fn data_size(_: *mut c_void) -> i32 { 0 }
    unsafe extern "system" fn is_array(_: *mut c_void) -> bool { false }
    unsafe extern "system" fn read(_: *mut c_void, _: i32, _: *mut c_void, _: u32) -> i32 {
        panic!("zero-length payload must not call AVS read")
    }

    #[test]
    fn flagged_avs_element_has_no_scalar_data() {
        assert!(is_element_kind(1));
        assert!(is_element_kind(47));
        assert!(is_element_kind(65));
        assert!(is_element_kind(193));
        assert!(!is_element_kind(6));
    }

    #[test]
    fn empty_binary_node_does_not_call_avs_read() {
        let api = AvsNodes { traversal, name, node_type, data_size, is_array, read };
        let (kind, value) = unsafe { api.value(std::ptr::null_mut(), 10, false) }.unwrap();
        assert_eq!(kind, "BINARY");
        assert_eq!(value, "");
    }
}
