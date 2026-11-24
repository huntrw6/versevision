//! A small protobuf message codec driven by the committed schema (`proto/descriptor.bin`).
//!
//! Why not `prost` structs or `prost-reflect`'s `DynamicMessage`: the Python app (upb) writes a float that is `-0.0` (the bit pattern is not
//! the default), keeps unknown fields and writes them last, and ProPresenter files carry both. Both Rust libraries treat `-0.0 == 0.0` as "not set"
//! and would drop it, so the files would differ byte for byte. This codec follows upb: known fields in field-number order, a scalar with
//! implicit presence is written unless its bits are all zero, unknown fields are kept and written after the known ones.
//! The schema (names, numbers, kinds, oneofs) still comes from `prost-reflect`'s descriptors.

use std::collections::BTreeMap;

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};
use thiserror::Error;

use crate::schema::pool;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PbError {
    #[error("protobuf: {0}")]
    Decode(String),
    #[error("protobuf: no message {0} in the schema")]
    NoMessage(String),
}

/// One field value. Enums are `I32`; the unsigned/signed/fixed variants share the Rust type of their width.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Msg(Message),
    List(Vec<Value>),
}

impl Value {
    pub fn as_msg(&self) -> Option<&Message> {
        match self {
            Value::Msg(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_msg_mut(&mut self) -> Option<&mut Message> {
        match self {
            Value::Msg(m) => Some(m),
            _ => None,
        }
    }
}

/// A protobuf message of the ProPresenter schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    desc: MessageDescriptor,
    fields: BTreeMap<u32, Value>,
    /// Raw `tag + payload` of every field the schema does not know (or knows with another wire type), in the order read.
    unknown: Vec<u8>,
}

// ---------------------------------------------------------------------------------------------------------------- wire reading

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.buf.len()
    }
    fn varint(&mut self) -> Result<u64, PbError> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or_else(|| PbError::Decode("truncated varint".into()))?;
            self.pos += 1;
            if shift < 64 {
                value |= u64::from(byte & 0x7f) << shift;
            }
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PbError::Decode("varint too long".into()))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], PbError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| PbError::Decode("truncated field".into()))?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn fixed32(&mut self) -> Result<u32, PbError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn fixed64(&mut self) -> Result<u64, PbError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn wire_type(kind: &Kind) -> u64 {
    match kind {
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => 1,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => 5,
        Kind::String | Kind::Bytes | Kind::Message(_) => 2,
        _ => 0,
    }
}

fn zigzag32(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}
fn zigzag64(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// One scalar of `kind` from its wire form (`raw` is the varint/fixed value, or unused for length-delimited kinds).
fn scalar_from_bits(kind: &Kind, bits: u64) -> Value {
    match kind {
        Kind::Bool => Value::Bool(bits != 0),
        Kind::Int32 | Kind::Enum(_) => Value::I32(bits as i32),
        Kind::Sint32 => Value::I32(zigzag32(bits as u32)),
        Kind::Sfixed32 => Value::I32(bits as u32 as i32),
        Kind::Int64 => Value::I64(bits as i64),
        Kind::Sint64 => Value::I64(zigzag64(bits)),
        Kind::Sfixed64 => Value::I64(bits as i64),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(bits as u32),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(bits),
        Kind::Float => Value::F32(f32::from_bits(bits as u32)),
        Kind::Double => Value::F64(f64::from_bits(bits)),
        Kind::String | Kind::Bytes | Kind::Message(_) => unreachable!("not a scalar"),
    }
}

fn read_scalar(kind: &Kind, reader: &mut Reader<'_>) -> Result<Value, PbError> {
    let bits = match wire_type(kind) {
        0 => reader.varint()?,
        1 => reader.fixed64()?,
        _ => u64::from(reader.fixed32()?),
    };
    Ok(scalar_from_bits(kind, bits))
}

/// A scalar that is the proto3 default (all bits zero) is not written unless the field tracks presence.
fn is_default_scalar(value: &Value) -> bool {
    match value {
        Value::Bool(b) => !b,
        Value::I32(v) => *v == 0,
        Value::I64(v) => *v == 0,
        Value::U32(v) => *v == 0,
        Value::U64(v) => *v == 0,
        Value::F32(v) => v.to_bits() == 0,
        Value::F64(v) => v.to_bits() == 0,
        Value::Str(s) => s.is_empty(),
        Value::Bytes(b) => b.is_empty(),
        Value::Msg(_) | Value::List(_) => false,
    }
}

fn write_scalar(kind: &Kind, value: &Value, out: &mut Vec<u8>) {
    match (kind, value) {
        (Kind::Bool, Value::Bool(v)) => put_varint(out, u64::from(*v)),
        (Kind::Int32 | Kind::Enum(_), Value::I32(v)) => put_varint(out, i64::from(*v) as u64),
        (Kind::Sint32, Value::I32(v)) => {
            put_varint(out, u64::from(((*v << 1) ^ (*v >> 31)) as u32))
        }
        (Kind::Sfixed32, Value::I32(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (Kind::Int64, Value::I64(v)) => put_varint(out, *v as u64),
        (Kind::Sint64, Value::I64(v)) => put_varint(out, ((*v << 1) ^ (*v >> 63)) as u64),
        (Kind::Sfixed64, Value::I64(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (Kind::Uint32, Value::U32(v)) => put_varint(out, u64::from(*v)),
        (Kind::Fixed32, Value::U32(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (Kind::Uint64, Value::U64(v)) => put_varint(out, *v),
        (Kind::Fixed64, Value::U64(v)) => out.extend_from_slice(&v.to_le_bytes()),
        (Kind::Float, Value::F32(v)) => out.extend_from_slice(&v.to_bits().to_le_bytes()),
        (Kind::Double, Value::F64(v)) => out.extend_from_slice(&v.to_bits().to_le_bytes()),
        _ => unreachable!("value {value:?} does not fit kind {kind:?}"),
    }
}

impl Message {
    /// An empty message of the named type (`rv.data.Presentation`, `rv.data.Template.Document`, ...).
    pub fn new_named(full_name: &str) -> Result<Message, PbError> {
        let desc = pool()
            .get_message_by_name(full_name)
            .ok_or_else(|| PbError::NoMessage(full_name.to_string()))?;
        Ok(Message::new(desc))
    }

    pub fn new(desc: MessageDescriptor) -> Message {
        Message {
            desc,
            fields: BTreeMap::new(),
            unknown: Vec::new(),
        }
    }

    pub fn decode_named(full_name: &str, bytes: &[u8]) -> Result<Message, PbError> {
        let mut message = Message::new_named(full_name)?;
        message.merge(bytes)?;
        Ok(message)
    }

    pub fn descriptor(&self) -> &MessageDescriptor {
        &self.desc
    }

    pub fn full_name(&self) -> &str {
        self.desc.full_name()
    }

    fn field(&self, name: &str) -> FieldDescriptor {
        self.desc
            .get_field_by_name(name)
            .unwrap_or_else(|| panic!("{} has no field {name}", self.desc.full_name()))
    }

    // ---------------------------------------------------------------------------------------------------------------- decode

    fn merge(&mut self, bytes: &[u8]) -> Result<(), PbError> {
        let mut reader = Reader { buf: bytes, pos: 0 };
        while !reader.eof() {
            let start = reader.pos;
            let tag = reader.varint()?;
            let (number, wire) = ((tag >> 3) as u32, tag & 7);
            if number == 0 {
                return Err(PbError::Decode("field number 0".into()));
            }
            let known = self.desc.get_field(number).filter(|f| {
                let expected = wire_type(&f.kind());
                wire == expected || (f.is_list() && wire == 2 && expected != 2)
            });
            match known {
                Some(field) => self.read_field(&field, wire, &mut reader)?,
                None => {
                    match wire {
                        0 => {
                            reader.varint()?;
                        }
                        1 => {
                            reader.take(8)?;
                        }
                        2 => {
                            let n = reader.varint()? as usize;
                            reader.take(n)?;
                        }
                        5 => {
                            reader.take(4)?;
                        }
                        other => {
                            return Err(PbError::Decode(format!("unsupported wire type {other}")))
                        }
                    }
                    self.unknown.extend_from_slice(&bytes[start..reader.pos]);
                }
            }
        }
        Ok(())
    }

    fn read_field(
        &mut self,
        field: &FieldDescriptor,
        wire: u64,
        reader: &mut Reader<'_>,
    ) -> Result<(), PbError> {
        if field.is_map() {
            return Err(PbError::Decode(format!(
                "map field {} is not supported",
                field.full_name()
            )));
        }
        let kind = field.kind();
        let number = field.number();
        if field.is_list() {
            let expected = wire_type(&kind);
            let mut items = Vec::new();
            if wire == 2 && expected != 2 {
                // packed
                let n = reader.varint()? as usize;
                let mut inner = Reader {
                    buf: reader.take(n)?,
                    pos: 0,
                };
                while !inner.eof() {
                    items.push(read_scalar(&kind, &mut inner)?);
                }
            } else {
                items.push(read_single(&kind, reader)?);
            }
            match self
                .fields
                .entry(number)
                .or_insert_with(|| Value::List(Vec::new()))
            {
                Value::List(list) => list.extend(items),
                _ => unreachable!("list field holds a list"),
            }
            return Ok(());
        }
        let value = read_single(&kind, reader)?;
        match value {
            Value::Msg(incoming) => {
                if let Some(oneof) = field.containing_oneof() {
                    for sibling in oneof.fields().filter(|s| s.number() != number) {
                        self.fields.remove(&sibling.number());
                    }
                }
                match self.fields.get_mut(&number) {
                    Some(Value::Msg(existing)) => existing.absorb(incoming),
                    _ => {
                        self.fields.insert(number, Value::Msg(incoming));
                    }
                }
            }
            scalar => self.set_value(field, scalar),
        }
        Ok(())
    }

    /// Protobuf merge of a second occurrence of a message field: scalars overwrite, repeated append, messages merge.
    fn absorb(&mut self, other: Message) {
        for (number, value) in other.fields {
            match (self.fields.get_mut(&number), value) {
                (Some(Value::Msg(mine)), Value::Msg(theirs)) => mine.absorb(theirs),
                (Some(Value::List(mine)), Value::List(theirs)) => mine.extend(theirs),
                (_, value) => {
                    self.fields.insert(number, value);
                }
            }
        }
        self.unknown.extend(other.unknown);
    }

    /// Store a scalar the way upb does: an implicit-presence field at its default value is simply absent.
    fn set_value(&mut self, field: &FieldDescriptor, value: Value) {
        let number = field.number();
        if let Some(oneof) = field.containing_oneof() {
            for sibling in oneof.fields().filter(|s| s.number() != number) {
                self.fields.remove(&sibling.number());
            }
        }
        if !field.supports_presence() && is_default_scalar(&value) {
            self.fields.remove(&number);
        } else {
            self.fields.insert(number, value);
        }
    }

    // ---------------------------------------------------------------------------------------------------------------- encode

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        for (number, value) in &self.fields {
            let field = self
                .desc
                .get_field(*number)
                .expect("a stored field is in the schema");
            let kind = field.kind();
            match value {
                Value::List(items) => {
                    if items.is_empty() {
                        continue;
                    }
                    if field.is_packed() {
                        let mut body = Vec::new();
                        for item in items {
                            write_scalar(&kind, item, &mut body);
                        }
                        put_varint(out, u64::from(*number) << 3 | 2);
                        put_varint(out, body.len() as u64);
                        out.extend_from_slice(&body);
                    } else {
                        for item in items {
                            write_single(*number, &kind, item, out);
                        }
                    }
                }
                single => write_single(*number, &kind, single, out),
            }
        }
        out.extend_from_slice(&self.unknown);
    }

    // ---------------------------------------------------------------------------------------------------------------- access

    /// The value of a field that is set (`None`: absent, or at its default for an implicit-presence field).
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(&self.field(name).number())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|v| !matches!(v, Value::List(l) if l.is_empty()))
    }

    pub fn clear(&mut self, name: &str) {
        let number = self.field(name).number();
        self.fields.remove(&number);
    }

    pub fn msg(&self, name: &str) -> Option<&Message> {
        self.get(name).and_then(Value::as_msg)
    }

    /// The submessage, created empty when it is not there yet (what assigning to a field of it does in Python).
    pub fn msg_mut(&mut self, name: &str) -> &mut Message {
        let field = self.field(name);
        let Kind::Message(sub) = field.kind() else {
            panic!("{name} is not a message field")
        };
        if let Some(oneof) = field.containing_oneof() {
            if !self.fields.contains_key(&field.number()) {
                for sibling in oneof.fields().filter(|s| s.number() != field.number()) {
                    self.fields.remove(&sibling.number());
                }
            }
        }
        self.fields
            .entry(field.number())
            .or_insert_with(|| Value::Msg(Message::new(sub)))
            .as_msg_mut()
            .expect("message field holds a message")
    }

    /// Replace a message field.
    pub fn set_msg(&mut self, name: &str, value: Message) {
        let field = self.field(name);
        if let Some(oneof) = field.containing_oneof() {
            for sibling in oneof.fields().filter(|s| s.number() != field.number()) {
                self.fields.remove(&sibling.number());
            }
        }
        self.fields.insert(field.number(), Value::Msg(value));
    }

    /// The items of a repeated message field.
    pub fn msgs(&self, name: &str) -> Vec<&Message> {
        match self.get(name) {
            Some(Value::List(items)) => items.iter().filter_map(Value::as_msg).collect(),
            _ => Vec::new(),
        }
    }

    pub fn msgs_mut(&mut self, name: &str) -> Vec<&mut Message> {
        let number = self.field(name).number();
        match self.fields.get_mut(&number) {
            Some(Value::List(items)) => items.iter_mut().filter_map(Value::as_msg_mut).collect(),
            _ => Vec::new(),
        }
    }

    pub fn len(&self, name: &str) -> usize {
        match self.get(name) {
            Some(Value::List(items)) => items.len(),
            _ => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.unknown.is_empty()
    }

    /// Append an empty message to a repeated field and return it (`field.add()`).
    pub fn add(&mut self, name: &str) -> &mut Message {
        let field = self.field(name);
        let Kind::Message(sub) = field.kind() else {
            panic!("{name} is not a message field")
        };
        let list = self
            .fields
            .entry(field.number())
            .or_insert_with(|| Value::List(Vec::new()));
        let Value::List(items) = list else {
            panic!("{name} is not repeated")
        };
        items.push(Value::Msg(Message::new(sub)));
        items
            .last_mut()
            .and_then(Value::as_msg_mut)
            .expect("just pushed")
    }

    pub fn push(&mut self, name: &str, message: Message) {
        let number = self.field(name).number();
        match self
            .fields
            .entry(number)
            .or_insert_with(|| Value::List(Vec::new()))
        {
            Value::List(items) => items.push(Value::Msg(message)),
            _ => panic!("{name} is not repeated"),
        }
    }

    /// `del message.field[:]`.
    pub fn clear_list(&mut self, name: &str) {
        self.clear(name);
    }

    pub fn set_str(&mut self, name: &str, value: &str) {
        let field = self.field(name);
        self.set_value(&field, Value::Str(value.to_string()));
    }
    pub fn set_bytes(&mut self, name: &str, value: Vec<u8>) {
        let field = self.field(name);
        self.set_value(&field, Value::Bytes(value));
    }
    pub fn set_i32(&mut self, name: &str, value: i32) {
        let field = self.field(name);
        self.set_value(&field, Value::I32(value));
    }
    /// An integer field of any width (`seconds`, `nanos`, ...).
    pub fn set_int(&mut self, name: &str, value: i64) {
        let field = self.field(name);
        let value = match field.kind() {
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Enum(_) => Value::I32(value as i32),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(value),
            Kind::Uint32 | Kind::Fixed32 => Value::U32(value as u32),
            Kind::Uint64 | Kind::Fixed64 => Value::U64(value as u64),
            other => panic!("{name} is not an integer field ({other:?})"),
        };
        self.set_value(&field, value);
    }
    pub fn set_f64(&mut self, name: &str, value: f64) {
        let field = self.field(name);
        self.set_value(&field, Value::F64(value));
    }

    pub fn str(&self, name: &str) -> &str {
        match self.get(name) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }
    pub fn bytes(&self, name: &str) -> &[u8] {
        match self.get(name) {
            Some(Value::Bytes(b)) => b,
            _ => &[],
        }
    }
    pub fn i64(&self, name: &str) -> i64 {
        match self.get(name) {
            Some(Value::I64(v)) => *v,
            Some(Value::I32(v)) => i64::from(*v),
            _ => 0,
        }
    }
    pub fn i32(&self, name: &str) -> i32 {
        match self.get(name) {
            Some(Value::I32(v)) => *v,
            _ => 0,
        }
    }
    pub fn u32(&self, name: &str) -> u32 {
        match self.get(name) {
            Some(Value::U32(v)) => *v,
            _ => 0,
        }
    }
    /// A float or double field as `f64` (a float widened exactly, as Python reads it).
    pub fn float(&self, name: &str) -> f64 {
        match self.get(name) {
            Some(Value::F32(v)) => f64::from(*v),
            Some(Value::F64(v)) => *v,
            _ => 0.0,
        }
    }

    /// Every set field in field-number order: the descriptor and the value (`ListFields()`).
    pub fn list_fields(&self) -> impl Iterator<Item = (FieldDescriptor, &Value)> {
        self.fields
            .iter()
            .filter(|(_, v)| !matches!(v, Value::List(l) if l.is_empty()))
            .map(|(n, v)| {
                (
                    self.desc
                        .get_field(*n)
                        .expect("stored field is in the schema"),
                    v,
                )
            })
    }

    /// Mutable `ListFields()`: the values can be changed in place.
    pub fn list_fields_mut(&mut self) -> impl Iterator<Item = (FieldDescriptor, &mut Value)> {
        let desc = self.desc.clone();
        self.fields
            .iter_mut()
            .filter(|(_, v)| !matches!(v, Value::List(l) if l.is_empty()))
            .map(move |(n, v)| {
                (
                    desc.get_field(*n).expect("stored field is in the schema"),
                    v,
                )
            })
    }

    /// Drop a field by number (used while walking the fields with `list_fields_mut`).
    pub fn remove_number(&mut self, number: u32) {
        self.fields.remove(&number);
    }

    /// Which message type a field holds (`None` for a scalar).
    pub fn field_message_type(field: &FieldDescriptor) -> Option<MessageDescriptor> {
        match field.kind() {
            Kind::Message(m) => Some(m),
            _ => None,
        }
    }

    /// The number of bytes of unknown fields kept.
    pub fn unknown_len(&self) -> usize {
        self.unknown.len()
    }
}

fn read_single(kind: &Kind, reader: &mut Reader<'_>) -> Result<Value, PbError> {
    match kind {
        Kind::String => {
            let n = reader.varint()? as usize;
            let bytes = reader.take(n)?;
            String::from_utf8(bytes.to_vec())
                .map(Value::Str)
                .map_err(|_| PbError::Decode("invalid UTF-8 in a string field".into()))
        }
        Kind::Bytes => {
            let n = reader.varint()? as usize;
            Ok(Value::Bytes(reader.take(n)?.to_vec()))
        }
        Kind::Message(desc) => {
            let n = reader.varint()? as usize;
            let body = reader.take(n)?;
            let mut message = Message::new(desc.clone());
            message.merge(body)?;
            Ok(Value::Msg(message))
        }
        scalar => read_scalar(scalar, reader),
    }
}

fn write_single(number: u32, kind: &Kind, value: &Value, out: &mut Vec<u8>) {
    match (kind, value) {
        (Kind::String, Value::Str(s)) => {
            put_varint(out, u64::from(number) << 3 | 2);
            put_varint(out, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        (Kind::Bytes, Value::Bytes(b)) => {
            put_varint(out, u64::from(number) << 3 | 2);
            put_varint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        (Kind::Message(_), Value::Msg(m)) => {
            let body = m.encode();
            put_varint(out, u64::from(number) << 3 | 2);
            put_varint(out, body.len() as u64);
            out.extend_from_slice(&body);
        }
        (scalar, value) => {
            put_varint(out, u64::from(number) << 3 | wire_type(scalar));
            write_scalar(scalar, value, out);
        }
    }
}
