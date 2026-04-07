mod encoding;
mod model;
mod writer;

// Re-export main types for convenience
pub use model::{Error, KeyValue, ProcessContext, Result, Value};
pub use writer::ProcessContextWriter;

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    struct PbProcessContext {
        #[prost(message, optional, tag = "1")]
        resource: Option<PbResource>,
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
    }

    #[test]
    fn test_encoding_roundtrip() {
        let ctx = ProcessContext::new()
            .with_resource("service.name", "my-service")
            .with_resource("service.version", "2.0.0")
            .with_resource("foo", "bar")
            .with_resource("baz", "qux");

        let encoded = encoding::encode(&ctx).unwrap();
        let decoded = PbProcessContext::decode(encoded.as_slice()).unwrap();

        let resource = decoded.resource.unwrap();
        assert_eq!(resource.attributes.len(), 4);
        assert_eq!(resource.attributes[0].key, "service.name");
        assert_eq!(
            resource.attributes[0].value.as_ref().unwrap().string_value,
            Some("my-service".to_string())
        );
    }
}
