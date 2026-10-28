//! Direct canonical JSON writer for `canonical_hash`.
//!
//! The bytes are exactly `canonical_json(&serde_json::to_value(value)?)`:
//! object keys sort by byte order (a repeated key keeps its last value, like
//! `serde_json::Map::insert`), arrays keep their order, scalars use
//! serde_json's own formatting (`f32` widens to `f64` first, as `to_value`
//! does). It skips the intermediate `Value` tree, which dominated the cost of
//! hashing large reports. Only map keys that `to_value` turns into JSON
//! strings without formatting floats are accepted; anything else is an error,
//! as it is for `to_value`.

use serde::ser::{self, Error as _, Impossible, Serialize};
use serde_json::Error;

pub(crate) fn canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    canonical_bytes_until(value, &|| false)
}

/// `canonical_bytes` that fails once `stop` returns true; `stop` is polled
/// before every array element.
pub(crate) fn canonical_bytes_until<T: Serialize + ?Sized>(
    value: &T,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut pool = Vec::new();
    value.serialize(Canonical {
        out: &mut out,
        pool: &mut pool,
        stop,
    })?;
    Ok(out)
}

/// Reusable scratch buffers, one per open object level.
type Pool = Vec<Vec<u8>>;

struct Canonical<'a> {
    out: &'a mut Vec<u8>,
    pool: &'a mut Pool,
    stop: &'a dyn Fn() -> bool,
}

fn scalar<T: Serialize + ?Sized>(out: &mut Vec<u8>, value: &T) -> Result<(), Error> {
    serde_json::to_writer(out, value)
}

impl<'a> Canonical<'a> {
    fn object(self, wrapper: Option<&'static str>) -> Object<'a> {
        let buffer = self.pool.pop().unwrap_or_default();
        Object {
            out: self.out,
            pool: self.pool,
            stop: self.stop,
            wrapper,
            buffer,
            entries: Vec::new(),
            pending_key: None,
        }
    }

    fn array(self, wrapper: Option<&'static str>) -> Result<Array<'a>, Error> {
        if let Some(variant) = wrapper {
            self.out.push(b'{');
            scalar(self.out, variant)?;
            self.out.push(b':');
        }
        self.out.push(b'[');
        Ok(Array {
            out: self.out,
            pool: self.pool,
            stop: self.stop,
            wrapper: wrapper.is_some(),
            first: true,
        })
    }
}

impl<'a> ser::Serializer for Canonical<'a> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Array<'a>;
    type SerializeTuple = Array<'a>;
    type SerializeTupleStruct = Array<'a>;
    type SerializeTupleVariant = Array<'a>;
    type SerializeMap = Object<'a>;
    type SerializeStruct = Object<'a>;
    type SerializeStructVariant = Object<'a>;

    fn serialize_bool(self, value: bool) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_i8(self, value: i8) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_i16(self, value: i16) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_i32(self, value: i32) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_i64(self, value: i64) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_i128(self, value: i128) -> Result<(), Error> {
        // `to_value` keeps only values that fit a JSON i64/u64 number.
        if let Ok(value) = i64::try_from(value) {
            scalar(self.out, &value)
        } else if let Ok(value) = u64::try_from(value) {
            scalar(self.out, &value)
        } else {
            Err(Error::custom("number out of range"))
        }
    }
    fn serialize_u8(self, value: u8) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_u16(self, value: u16) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_u32(self, value: u32) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_u64(self, value: u64) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_u128(self, value: u128) -> Result<(), Error> {
        let value = u64::try_from(value).map_err(|_| Error::custom("number out of range"))?;
        scalar(self.out, &value)
    }
    fn serialize_f32(self, value: f32) -> Result<(), Error> {
        scalar(self.out, &f64::from(value))
    }
    fn serialize_f64(self, value: f64) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_char(self, value: char) -> Result<(), Error> {
        scalar(self.out, &value)
    }
    fn serialize_str(self, value: &str) -> Result<(), Error> {
        scalar(self.out, value)
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Error> {
        let mut array = self.array(None)?;
        for byte in value {
            ser::SerializeSeq::serialize_element(&mut array, byte)?;
        }
        ser::SerializeSeq::end(array)
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.serialize_unit()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.out.extend_from_slice(b"null");
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.out.push(b'{');
        scalar(self.out, variant)?;
        self.out.push(b':');
        value.serialize(Canonical {
            out: &mut *self.out,
            pool: &mut *self.pool,
            stop: self.stop,
        })?;
        self.out.push(b'}');
        Ok(())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Array<'a>, Error> {
        self.array(None)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Array<'a>, Error> {
        self.array(None)
    }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Array<'a>, Error> {
        self.array(None)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Array<'a>, Error> {
        self.array(Some(variant))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Object<'a>, Error> {
        Ok(self.object(None))
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Object<'a>, Error> {
        Ok(self.object(None))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Object<'a>, Error> {
        Ok(self.object(Some(variant)))
    }
}

struct Array<'a> {
    out: &'a mut Vec<u8>,
    pool: &'a mut Pool,
    stop: &'a dyn Fn() -> bool,
    wrapper: bool,
    first: bool,
}

impl Array<'_> {
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        if (self.stop)() {
            return Err(Error::custom("canonical serialization stopped"));
        }
        if !self.first {
            self.out.push(b',');
        }
        self.first = false;
        value.serialize(Canonical {
            out: &mut *self.out,
            pool: &mut *self.pool,
            stop: self.stop,
        })
    }

    fn finish(self) -> Result<(), Error> {
        self.out.push(b']');
        if self.wrapper {
            self.out.push(b'}');
        }
        Ok(())
    }
}

impl ser::SerializeSeq for Array<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTuple for Array<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for Array<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for Array<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

/// Collects `key -> value bytes` in one scratch buffer, then writes the
/// entries sorted by key.
struct Object<'a> {
    out: &'a mut Vec<u8>,
    pool: &'a mut Pool,
    stop: &'a dyn Fn() -> bool,
    wrapper: Option<&'static str>,
    buffer: Vec<u8>,
    entries: Vec<(Key, std::ops::Range<usize>)>,
    pending_key: Option<String>,
}

enum Key {
    Static(&'static str),
    Owned(String),
}

impl Key {
    fn as_str(&self) -> &str {
        match self {
            Self::Static(key) => key,
            Self::Owned(key) => key,
        }
    }
}

impl Object<'_> {
    fn entry<T: Serialize + ?Sized>(&mut self, key: Key, value: &T) -> Result<(), Error> {
        let start = self.buffer.len();
        value.serialize(Canonical {
            out: &mut self.buffer,
            pool: &mut *self.pool,
            stop: self.stop,
        })?;
        self.entries.push((key, start..self.buffer.len()));
        Ok(())
    }

    fn finish(mut self) -> Result<(), Error> {
        // Stable sort, then keep the last of equal keys: `Map::insert` replaces.
        self.entries
            .sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        if let Some(variant) = self.wrapper {
            self.out.push(b'{');
            scalar(self.out, variant)?;
            self.out.push(b':');
        }
        self.out.push(b'{');
        let mut first = true;
        for (index, (key, range)) in self.entries.iter().enumerate() {
            if self
                .entries
                .get(index + 1)
                .is_some_and(|next| next.0.as_str() == key.as_str())
            {
                continue;
            }
            if !first {
                self.out.push(b',');
            }
            first = false;
            scalar(self.out, key.as_str())?;
            self.out.push(b':');
            self.out.extend_from_slice(&self.buffer[range.clone()]);
        }
        self.out.push(b'}');
        if self.wrapper.is_some() {
            self.out.push(b'}');
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.clear();
        self.pool.push(buffer);
        Ok(())
    }
}

impl ser::SerializeMap for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.pending_key = Some(key.serialize(MapKey)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| Error::custom("map value without a key"))?;
        self.entry(Key::Owned(key), value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStruct for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.entry(Key::Static(key), value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.entry(Key::Static(key), value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

/// Map keys as `serde_json::to_value` stores them.
struct MapKey;

fn key_must_be_a_string() -> Error {
    Error::custom("key must be a string")
}

impl ser::Serializer for MapKey {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_bool(self, value: bool) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_i8(self, value: i8) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_i16(self, value: i16) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_i32(self, value: i32) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_i64(self, value: i64) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_i128(self, value: i128) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_u8(self, value: u8) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_u16(self, value: u16) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_u32(self, value: u32) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_u64(self, value: u64) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_u128(self, value: u128) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_f32(self, _value: f32) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_f64(self, _value: f64) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_char(self, value: char) -> Result<String, Error> {
        Ok(value.to_string())
    }
    fn serialize_str(self, value: &str) -> Result<String, Error> {
        Ok(value.to_owned())
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_none(self) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_unit(self) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String, Error> {
        Ok(variant.to_owned())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<String, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        Err(key_must_be_a_string())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        Err(key_must_be_a_string())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use serde::Serialize;
    use serde_json::json;

    use super::canonical_bytes;
    use crate::canonical_json;

    fn via_value<T: Serialize>(value: &T) -> String {
        canonical_json(&serde_json::to_value(value).expect("to_value"))
    }

    fn assert_same<T: Serialize>(value: &T) {
        let direct = String::from_utf8(canonical_bytes(value).expect("direct")).expect("utf-8");
        assert_eq!(direct, via_value(value));
    }

    #[derive(Serialize)]
    #[serde(rename_all = "snake_case")]
    enum External {
        Unit,
        Newtype(u32),
        Tuple(i8, String),
        Struct { zeta: bool, alpha: Option<f32> },
    }

    #[derive(Serialize)]
    #[serde(tag = "state", rename_all = "snake_case")]
    enum Internal {
        Available { value: String, fact_ref: String },
        Missing { reasons: Vec<String> },
    }

    #[derive(Serialize)]
    #[serde(untagged)]
    enum Untagged {
        Text(String),
        Number(i64),
    }

    #[derive(Serialize)]
    struct Unit;

    #[derive(Serialize)]
    struct Wrapper(String);

    #[derive(Serialize)]
    struct Inner {
        b: Vec<u8>,
        a: char,
    }

    #[derive(Serialize)]
    struct Flattened {
        zz: u64,
        #[serde(flatten)]
        inner: Inner,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped: Option<u8>,
    }

    #[derive(Serialize)]
    struct Everything {
        z_external: Vec<External>,
        internal: Vec<Internal>,
        untagged: Vec<Untagged>,
        unit: Unit,
        wrapper: Wrapper,
        flattened: Flattened,
        floats: Vec<f64>,
        small_float: f32,
        ints: (i64, u64, i128, u128),
        text: &'static str,
        escapes: String,
        sorted_map: BTreeMap<String, Option<String>>,
        hashed_map: HashMap<u32, bool>,
        nothing: Option<()>,
        bytes: serde_bytes_like::Bytes,
        value: serde_json::Value,
    }

    mod serde_bytes_like {
        pub struct Bytes(pub Vec<u8>);
        impl serde::Serialize for Bytes {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(&self.0)
            }
        }
    }

    #[test]
    fn direct_bytes_match_canonical_json_of_to_value() {
        let value = Everything {
            z_external: vec![
                External::Unit,
                External::Newtype(7),
                External::Tuple(-3, "t".into()),
                External::Struct {
                    zeta: true,
                    alpha: Some(0.1),
                },
                External::Struct {
                    zeta: false,
                    alpha: None,
                },
            ],
            internal: vec![
                Internal::Available {
                    value: "1".into(),
                    fact_ref: "f".into(),
                },
                Internal::Missing { reasons: vec![] },
            ],
            untagged: vec![Untagged::Text("x".into()), Untagged::Number(-9)],
            unit: Unit,
            wrapper: Wrapper("w".into()),
            flattened: Flattened {
                zz: u64::MAX,
                inner: Inner {
                    b: vec![0, 255],
                    a: '\u{1F600}',
                },
                skipped: None,
            },
            floats: vec![0.0, -0.0, 1.5, 1e21, 1e-7, f64::NAN, f64::INFINITY, 0.1],
            small_float: 0.1,
            ints: (i64::MIN, u64::MAX, -5, 6),
            text: "plain",
            escapes: "quote\" backslash\\ newline\n tab\t nul\u{0} del\u{7f} é 中".into(),
            sorted_map: BTreeMap::from([
                ("b".into(), None),
                ("a".into(), Some("1".into())),
                ("B".into(), Some(String::new())),
                ("é".into(), None),
            ]),
            hashed_map: HashMap::from([(10, true), (2, false), (33, true)]),
            nothing: None,
            bytes: serde_bytes_like::Bytes(vec![1, 2, 3]),
            value: json!({"z": [1, {"y": null, "x": 2.5}], "a": "s", "m": {}}),
        };
        assert_same(&value);
        assert_same(&json!([]));
        assert_same(&json!({}));
        assert_same(&"top-level string");
        assert_same(&Option::<u8>::None);
    }

    #[test]
    fn repeated_keys_keep_the_last_value_like_map_insert() {
        struct Repeated;
        impl Serialize for Repeated {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("k", &1)?;
                map.serialize_entry("a", &0)?;
                map.serialize_entry("k", &2)?;
                map.end()
            }
        }
        assert_same(&Repeated);
        assert_eq!(canonical_bytes(&Repeated).unwrap(), br#"{"a":0,"k":2}"#);
    }

    #[test]
    fn non_string_keys_are_rejected_like_to_value() {
        let map = BTreeMap::from([(vec![1u8], 1)]);
        assert!(canonical_bytes(&map).is_err());
        assert!(serde_json::to_value(&map).is_err());
    }
}

#[cfg(test)]
mod cancellable_hash_tests {
    use std::cell::Cell;

    use serde_json::json;

    use crate::{canonical_hash, canonical_hash_cancellable};

    #[test]
    fn an_unstopped_run_hashes_like_canonical_hash_and_a_stop_gives_none() {
        // 2,000 array elements plus the 4 KiB digest chunks.
        let value = json!({
            "cells": (0..2_000).map(|i| json!({"i": i, "pad": "x".repeat(64)})).collect::<Vec<_>>(),
        });
        let polls = Cell::new(0usize);
        let counting = || {
            polls.set(polls.get() + 1);
            false
        };
        assert_eq!(
            canonical_hash_cancellable(&value, &counting).as_deref(),
            Some(canonical_hash(&value).as_str())
        );
        let total = polls.get();
        assert!(total > 2_000, "{total} polls");
        for after in [1, total / 2, total] {
            let polls = Cell::new(0usize);
            let stop = || {
                polls.set(polls.get() + 1);
                polls.get() >= after
            };
            assert_eq!(
                canonical_hash_cancellable(&value, &stop),
                None,
                "poll {after}"
            );
            assert_eq!(polls.get(), after, "stops at the raised poll");
        }
    }
}
