//! A minimal protobuf implementation for encoding/decoding OTEL process-context.
//! Supports string, int64, and kvlist value types.
//!
//! This could be replaced with a full protobuf library (e.g. prost) if richer
//! type support or stronger conformance guarantees are needed.

use super::model::{Error, KeyValue, ProcessContext, Result, Value, KEY_VALUE_LIMIT};

/// Wire type for varint fields (int64, uint64, int32, etc.)
const WIRE_TYPE_VARINT: u8 = 0;
/// Wire type for length-delimited fields (strings, bytes, nested messages)
const WIRE_TYPE_LEN: u8 = 2;

// =============================================================================
// Varint encoding/decoding
// =============================================================================

/// Write a varint to the buffer (supports values up to UINT14_MAX)
fn write_varint(buf: &mut Vec<u8>, value: u16) {
    if value < 128 {
        buf.push(value as u8);
    } else {
        buf.push((value & 0x7F) as u8 | 0x80);
        buf.push((value >> 7) as u8);
    }
}

/// Write an i64 as a varint. For positive values this is efficient.
/// Negative values require 10 bytes (two's complement), but we don't use them.
fn write_varint_i64(buf: &mut Vec<u8>, value: i64) {
    debug_assert!(value >= 0, "negative int64 values not supported");
    let mut v = value as u64;
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

// =============================================================================
// Tag encoding/decoding
// =============================================================================

/// Write a protobuf tag with wire type VARINT
fn write_tag_varint(buf: &mut Vec<u8>, field_number: u8) {
    buf.push((field_number << 3) | WIRE_TYPE_VARINT);
}

/// Write a protobuf tag with wire type LEN (length-delimited)
fn write_tag_len(buf: &mut Vec<u8>, field_number: u8) {
    buf.push((field_number << 3) | WIRE_TYPE_LEN);
}

// =============================================================================
// String encoding
// =============================================================================

/// Write a protobuf string (length + bytes, without tag)
fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_varint(buf, s.len() as u16);
    buf.extend_from_slice(s.as_bytes());
}

// =============================================================================
// AnyValue encoding
// =============================================================================

/// Encode an AnyValue message to bytes
fn encode_anyvalue(value: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    match value {
        Value::String(s) => {
            // string_value = field 1, wire type LEN
            write_tag_len(&mut buf, 1);
            write_string(&mut buf, s);
        }
        Value::Int(i) => {
            // int_value = field 3, wire type VARINT
            write_tag_varint(&mut buf, 3);
            write_varint_i64(&mut buf, *i);
        }
        Value::Array(values) => {
            // array_value = field 5, wire type LEN
            let array_bytes = encode_arrayvalue(values);
            write_tag_len(&mut buf, 5);
            write_varint(&mut buf, array_bytes.len() as u16);
            buf.extend(array_bytes);
        }
        Value::KvList(kvs) => {
            // kvlist_value = field 6, wire type LEN
            let kvlist_bytes = encode_kvlist(kvs);
            write_tag_len(&mut buf, 6);
            write_varint(&mut buf, kvlist_bytes.len() as u16);
            buf.extend(kvlist_bytes);
        }
    }
    buf
}

/// Encode an ArrayValue message to bytes
fn encode_arrayvalue(values: &[Value]) -> Vec<u8> {
    let mut buf = Vec::new();
    for val in values {
        // ArrayValue.values = field 1, wire type LEN
        let val_bytes = encode_anyvalue(val);
        write_tag_len(&mut buf, 1);
        write_varint(&mut buf, val_bytes.len() as u16);
        buf.extend(val_bytes);
    }
    buf
}

/// Encode a KeyValueList message to bytes
fn encode_kvlist(kvs: &[KeyValue]) -> Vec<u8> {
    let mut buf = Vec::new();
    for kv in kvs {
        // KeyValueList.values = field 1, wire type LEN
        let kv_bytes = encode_keyvalue(kv);
        write_tag_len(&mut buf, 1);
        write_varint(&mut buf, kv_bytes.len() as u16);
        buf.extend(kv_bytes);
    }
    buf
}

/// Encode a KeyValue message to bytes
fn encode_keyvalue(kv: &KeyValue) -> Vec<u8> {
    let mut buf = Vec::new();

    // KeyValue.key = field 1, wire type LEN
    write_tag_len(&mut buf, 1);
    write_string(&mut buf, &kv.key);

    // KeyValue.value = field 2, wire type LEN (AnyValue message)
    let anyvalue_bytes = encode_anyvalue(&kv.value);
    write_tag_len(&mut buf, 2);
    write_varint(&mut buf, anyvalue_bytes.len() as u16);
    buf.extend(anyvalue_bytes);

    buf
}

// =============================================================================
// Validation
// =============================================================================

/// Validate a value recursively
fn validate_value(key: &str, value: &Value) -> Result<()> {
    match value {
        Value::String(s) => {
            if s.len() > KEY_VALUE_LIMIT {
                return Err(Error::StringTooLong {
                    field: format!("value for '{}'", key),
                    len: s.len(),
                });
            }
        }
        Value::Int(_) => {}
        Value::Array(values) => {
            for val in values {
                validate_value(key, val)?;
            }
        }
        Value::KvList(kvs) => {
            for kv in kvs {
                validate_kv(kv)?;
            }
        }
    }
    Ok(())
}

/// Validate a key-value pair
fn validate_kv(kv: &KeyValue) -> Result<()> {
    if kv.key.len() > KEY_VALUE_LIMIT {
        return Err(Error::StringTooLong {
            field: kv.key.clone(),
            len: kv.key.len(),
        });
    }
    validate_value(&kv.key, &kv.value)
}

// =============================================================================
// Main encode function
// =============================================================================

/// Encode a ProcessContext to protobuf bytes.
///
/// The payload is a ProcessContext message:
///   field 1 (LEN): Resource { field 1 (LEN, repeated): KeyValue }
///   field 2 (LEN, repeated): KeyValue extra_attributes
pub fn encode(ctx: &ProcessContext) -> Result<Vec<u8>> {
    // Validate all resources
    for kv in &ctx.resources {
        validate_kv(kv)?;
    }
    // Validate all extra attributes
    for kv in &ctx.extra_attributes {
        validate_kv(kv)?;
    }

    let mut buf = Vec::new();

    // Encode Resource as field 1 of ProcessContext
    if !ctx.resources.is_empty() {
        let mut resource_buf = Vec::new();
        for kv in &ctx.resources {
            let kv_bytes = encode_keyvalue(kv);
            write_tag_len(&mut resource_buf, 1); // Resource.attributes = field 1
            write_varint(&mut resource_buf, kv_bytes.len() as u16);
            resource_buf.extend(kv_bytes);
        }
        write_tag_len(&mut buf, 1); // ProcessContext.resource = field 1
        write_varint(&mut buf, resource_buf.len() as u16);
        buf.extend(resource_buf);
    }

    // Encode extra_attributes as field 2 of ProcessContext (repeated)
    for kv in &ctx.extra_attributes {
        let kv_bytes = encode_keyvalue(kv);
        write_tag_len(&mut buf, 2); // ProcessContext.extra_attributes = field 2
        write_varint(&mut buf, kv_bytes.len() as u16);
        buf.extend(kv_bytes);
    }

    Ok(buf)
}

// =============================================================================
// Tests — uses prost to decode, validating the mini encoder produces real protobuf
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    // Prost-decoded OTel proto types (mirrors the standard OTel protobuf schema)

    #[derive(Clone, PartialEq, Message)]
    struct PbProcessContext {
        #[prost(message, optional, tag = "1")]
        resource: Option<PbResource>,
        #[prost(message, repeated, tag = "2")]
        extra_attributes: Vec<PbKeyValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    struct PbResource {
        #[prost(message, repeated, tag = "1")]
        attributes: Vec<PbKeyValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    struct PbKeyValue {
        #[prost(string, tag = "1")]
        key: String,
        #[prost(message, optional, tag = "2")]
        value: Option<PbAnyValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    struct PbAnyValue {
        #[prost(string, optional, tag = "1")]
        string_value: Option<String>,
        #[prost(int64, optional, tag = "3")]
        int_value: Option<i64>,
        #[prost(message, optional, tag = "5")]
        array_value: Option<PbArrayValue>,
        #[prost(message, optional, tag = "6")]
        kvlist_value: Option<PbKeyValueList>,
    }

    #[derive(Clone, PartialEq, Message)]
    struct PbArrayValue {
        #[prost(message, repeated, tag = "1")]
        values: Vec<PbAnyValue>,
    }

    #[derive(Clone, PartialEq, Message)]
    struct PbKeyValueList {
        #[prost(message, repeated, tag = "1")]
        values: Vec<PbKeyValue>,
    }

    /// Convert a prost-decoded AnyValue back to our model Value
    fn pb_to_value(av: &PbAnyValue) -> Value {
        if let Some(s) = &av.string_value {
            Value::String(s.clone())
        } else if let Some(i) = av.int_value {
            Value::Int(i)
        } else if let Some(arr) = &av.array_value {
            Value::Array(arr.values.iter().map(pb_to_value).collect())
        } else if let Some(kvl) = &av.kvlist_value {
            Value::KvList(
                kvl.values
                    .iter()
                    .map(|kv| KeyValue::new(&kv.key, pb_to_value(kv.value.as_ref().unwrap())))
                    .collect(),
            )
        } else {
            panic!("empty AnyValue")
        }
    }

    /// Decode encoded bytes via prost and convert back to our ProcessContext
    fn prost_decode(data: &[u8]) -> ProcessContext {
        let pb = PbProcessContext::decode(data).expect("prost failed to decode");
        let mut ctx = ProcessContext::new();
        if let Some(resource) = &pb.resource {
            for kv in &resource.attributes {
                ctx.resources.push(KeyValue::new(
                    &kv.key,
                    pb_to_value(kv.value.as_ref().unwrap()),
                ));
            }
        }
        for kv in &pb.extra_attributes {
            ctx.extra_attributes.push(KeyValue::new(
                &kv.key,
                pb_to_value(kv.value.as_ref().unwrap()),
            ));
        }
        ctx
    }

    #[test]
    fn test_roundtrip_string_values() {
        let ctx = ProcessContext::new()
            .with_resource("service.name", "my-service")
            .with_resource("service.version", "1.2.3")
            .with_resource("deployment.environment", "production");

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_int_values() {
        let ctx = ProcessContext::new()
            .with_resource("process.pid", Value::Int(1234))
            .with_resource("host.cpu.count", Value::Int(8));

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_kvlist() {
        let kvlist = vec![
            KeyValue::string("0", "http_route"),
            KeyValue::string("1", "http_method"),
            KeyValue::string("2", "user_id"),
        ];
        let ctx = ProcessContext::new()
            .with_resource("some.kvlist", Value::KvList(kvlist));

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_array() {
        let ctx = ProcessContext::new().with_resource(
            "some.array",
            Value::Array(vec![
                Value::String("http_route".to_string()),
                Value::String("http_method".to_string()),
                Value::String("user_id".to_string()),
            ]),
        );

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_extra_attributes() {
        let ctx = ProcessContext::new()
            .with_extra_attribute("threadlocal.schema_version", "tlsdesc_v1_dev")
            .with_extra_attribute("some.int_attr", Value::Int(64));

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_resources_and_extra_attributes() {
        let ctx = ProcessContext::new()
            .with_resource("service.name", "test-service")
            .with_extra_attribute("threadlocal.schema_version", "tlsdesc_v1_dev")
            .with_extra_attribute("some.int_attr", Value::Int(64))
            .with_extra_attribute(
                "threadlocal.attribute_key_map",
                Value::Array(vec![
                    Value::String("http_route".to_string()),
                    Value::String("http_method".to_string()),
                    Value::String("user_id".to_string()),
                ]),
            );

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_empty_resources() {
        let ctx = ProcessContext::new()
            .with_extra_attribute("foo", "bar");

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_roundtrip_empty_extra_attributes() {
        let ctx = ProcessContext::new()
            .with_resource("service.name", "test");

        let encoded = encode(&ctx).unwrap();
        assert_eq!(ctx, prost_decode(&encoded));
    }

    #[test]
    fn test_string_too_long() {
        let long_string = "x".repeat(KEY_VALUE_LIMIT + 1);
        let ctx = ProcessContext::new().with_resource("key", long_string.clone());
        assert!(matches!(encode(&ctx), Err(Error::StringTooLong { .. })));

        let ctx2 = ProcessContext::new().with_extra_attribute("key", long_string);
        assert!(matches!(encode(&ctx2), Err(Error::StringTooLong { .. })));
    }

    #[test]
    fn test_varint_encoding() {
        // Test single-byte varint
        let mut buf = Vec::new();
        write_varint(&mut buf, 127);
        assert_eq!(buf, vec![127]);

        // Test two-byte varint
        buf.clear();
        write_varint(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        // Test larger two-byte varint
        buf.clear();
        write_varint(&mut buf, 300);
        assert_eq!(buf, vec![0xAC, 0x02]);
    }

    #[test]
    fn test_varint_i64_encoding() {
        let mut buf = Vec::new();
        write_varint_i64(&mut buf, 1);
        assert_eq!(buf, vec![0x01]);

        buf.clear();
        write_varint_i64(&mut buf, 127);
        assert_eq!(buf, vec![0x7F]);

        buf.clear();
        write_varint_i64(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        buf.clear();
        write_varint_i64(&mut buf, 512);
        assert_eq!(buf, vec![0x80, 0x04]);
    }
}
