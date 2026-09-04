use panel_errors::{ErrorCode, PanelError, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};

pub const JSON_SNAPSHOT_RECORD_FORMAT_V1: u32 = 1;

/// Version-specific snapshot envelope codec.
///
/// Payloads cross this port as serialized JSON bytes, keeping the trait
/// independent of active/prepared record types and future store additions.
/// Encoded records must remain JSON objects with a top-level unsigned
/// `format_version`, which lets the registry select a decoder without knowing
/// any version-specific payload shape.
pub trait SnapshotRecordCodec: Send + Sync {
    fn format_version(&self) -> u32;
    fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>>;
    fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>>;
}

#[derive(Default)]
pub struct JsonSnapshotRecordCodecV1;

#[derive(Deserialize)]
struct DiskRecordHeader {
    format_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedDiskRecord {
    format_version: u32,
    payload: serde_json::Value,
}

impl SnapshotRecordCodec for JsonSnapshotRecordCodecV1 {
    fn format_version(&self) -> u32 {
        JSON_SNAPSHOT_RECORD_FORMAT_V1
    }

    fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
        serde_json::from_slice::<serde_json::Value>(payload).map_err(|error| {
            PanelError::internal("serialize v1 snapshot payload").with_source(error)
        })?;
        let mut record =
            format!("{{\"format_version\":{JSON_SNAPSHOT_RECORD_FORMAT_V1},\"payload\":")
                .into_bytes();
        record.extend_from_slice(payload);
        record.push(b'}');
        Ok(record)
    }

    fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>> {
        let disk: OwnedDiskRecord = serde_json::from_slice(record).map_err(|error| {
            PanelError::corrupt_state("invalid v1 snapshot record").with_source(error)
        })?;
        if disk.format_version != JSON_SNAPSHOT_RECORD_FORMAT_V1 {
            return Err(PanelError::corrupt_state(
                "v1 snapshot codec received a different format version",
            ));
        }
        serde_json::to_vec(&disk.payload)
            .map_err(|error| PanelError::internal("decode v1 snapshot payload").with_source(error))
    }
}

pub struct SnapshotRecordCodecRegistry {
    writer: Arc<dyn SnapshotRecordCodec>,
    writer_version: u32,
    readers: BTreeMap<u32, Arc<dyn SnapshotRecordCodec>>,
}

impl SnapshotRecordCodecRegistry {
    pub fn new(writer: Arc<dyn SnapshotRecordCodec>) -> Result<Self> {
        let version = contain_codec_panic("snapshot writer panicked during registration", || {
            writer.format_version()
        })?;
        validate_format_version(version)?;
        let mut readers = BTreeMap::new();
        readers.insert(version, Arc::clone(&writer));
        Ok(Self {
            writer,
            writer_version: version,
            readers,
        })
    }

    pub fn with_reader(mut self, reader: Arc<dyn SnapshotRecordCodec>) -> Result<Self> {
        let version = contain_codec_panic("snapshot reader panicked during registration", || {
            reader.format_version()
        })?;
        validate_format_version(version)?;
        if self.readers.insert(version, reader).is_some() {
            return Err(PanelError::conflict(format!(
                "snapshot record codec version {version} is already registered"
            )));
        }
        Ok(self)
    }

    pub fn writer_version(&self) -> u32 {
        self.writer_version
    }

    pub fn readable_versions(&self) -> impl Iterator<Item = u32> + '_ {
        self.readers.keys().copied()
    }

    pub fn encode<T: Serialize>(&self, payload: &T) -> Result<Vec<u8>> {
        let payload = serde_json::to_vec(payload).map_err(|error| {
            PanelError::internal("serialize snapshot payload").with_source(error)
        })?;
        let record = contain_codec_panic("snapshot record codec panicked while encoding", || {
            self.writer.encode_payload(&payload)
        })??;
        let header: DiskRecordHeader = serde_json::from_slice(&record).map_err(|error| {
            PanelError::internal("snapshot record codec produced an invalid envelope")
                .with_source(error)
        })?;
        if header.format_version != self.writer_version {
            return Err(PanelError::internal(format!(
                "snapshot record codec declared version {} but encoded version {}",
                self.writer_version, header.format_version
            )));
        }
        Ok(record)
    }

    pub fn decode<T: DeserializeOwned>(&self, record: &[u8]) -> Result<T> {
        self.decode_with_max_payload_bytes(record, u64::MAX)
    }

    /// Decodes a record while bounding the bytes produced by a versioned
    /// reader before deserialization. The bound is deliberately applied after
    /// the reader returns: a future codec may legitimately expand a compact
    /// representation, but it must not be able to bypass the store's resource
    /// policy through that expansion.
    pub fn decode_with_max_payload_bytes<T: DeserializeOwned>(
        &self,
        record: &[u8],
        max_payload_bytes: u64,
    ) -> Result<T> {
        let header: DiskRecordHeader = serde_json::from_slice(record).map_err(|error| {
            PanelError::corrupt_state("invalid snapshot record envelope").with_source(error)
        })?;
        let version = header.format_version;
        let codec = self.readers.get(&version).ok_or_else(|| {
            PanelError::new(
                ErrorCode::UNSUPPORTED_CAPABILITY,
                format!("snapshot store format {version} is not supported"),
            )
        })?;
        let payload =
            contain_codec_panic("snapshot record codec panicked while decoding", || {
                codec.decode_payload(record)
            })??;
        let payload_bytes = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        if payload_bytes > max_payload_bytes {
            return Err(PanelError::resource_exhausted(format!(
                "decoded snapshot payload exceeds the {max_payload_bytes} byte limit"
            )));
        }
        serde_json::from_slice(&payload).map_err(|error| {
            PanelError::corrupt_state("invalid snapshot record payload").with_source(error)
        })
    }
}

fn contain_codec_panic<T>(message: &'static str, operation: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(operation)).map_err(|_| PanelError::internal(message))
}

fn validate_format_version(version: u32) -> Result<()> {
    if version == 0 {
        return Err(PanelError::invalid_argument(
            "snapshot record codec version must be non-zero",
        ));
    }
    Ok(())
}

impl Default for SnapshotRecordCodecRegistry {
    fn default() -> Self {
        Self::new(Arc::new(JsonSnapshotRecordCodecV1))
            .expect("the built-in v1 snapshot codec registry is valid")
    }
}

impl fmt::Debug for SnapshotRecordCodecRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotRecordCodecRegistry")
            .field("writer_version", &self.writer_version())
            .field(
                "readable_versions",
                &self.readable_versions().collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct JsonSnapshotRecordCodecV2;

    impl SnapshotRecordCodec for JsonSnapshotRecordCodecV2 {
        fn format_version(&self) -> u32 {
            2
        }

        fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
            let value: serde_json::Value = serde_json::from_slice(payload).unwrap();
            Ok(serde_json::to_vec(&serde_json::json!({
                "format_version": 2,
                "payload": value,
            }))
            .unwrap())
        }

        fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>> {
            let disk: OwnedDiskRecord = serde_json::from_slice(record).unwrap();
            Ok(serde_json::to_vec(&disk.payload).unwrap())
        }
    }

    struct MutableVersionCodec {
        next_version: AtomicU32,
        encoded_version: u32,
    }

    enum PanicPoint {
        Registration,
        Encode,
        Decode,
    }

    struct PanickingCodec(PanicPoint);

    impl SnapshotRecordCodec for PanickingCodec {
        fn format_version(&self) -> u32 {
            if matches!(self.0, PanicPoint::Registration) {
                panic!("registration panic");
            }
            9
        }

        fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
            if matches!(self.0, PanicPoint::Encode) {
                panic!("encode panic");
            }
            let payload: serde_json::Value = serde_json::from_slice(payload).unwrap();
            Ok(serde_json::to_vec(&serde_json::json!({
                "format_version": 9,
                "payload": payload,
            }))
            .unwrap())
        }

        fn decode_payload(&self, _record: &[u8]) -> Result<Vec<u8>> {
            if matches!(self.0, PanicPoint::Decode) {
                panic!("decode panic");
            }
            Ok(br#""decoded""#.to_vec())
        }
    }

    impl MutableVersionCodec {
        fn new(initial_version: u32, encoded_version: u32) -> Self {
            Self {
                next_version: AtomicU32::new(initial_version),
                encoded_version,
            }
        }
    }

    impl SnapshotRecordCodec for MutableVersionCodec {
        fn format_version(&self) -> u32 {
            self.next_version.fetch_add(1, Ordering::Relaxed)
        }

        fn encode_payload(&self, payload: &[u8]) -> Result<Vec<u8>> {
            let payload: serde_json::Value = serde_json::from_slice(payload).unwrap();
            Ok(serde_json::to_vec(&serde_json::json!({
                "format_version": self.encoded_version,
                "payload": payload,
            }))
            .unwrap())
        }

        fn decode_payload(&self, record: &[u8]) -> Result<Vec<u8>> {
            let disk: OwnedDiskRecord = serde_json::from_slice(record).unwrap();
            Ok(serde_json::to_vec(&disk.payload).unwrap())
        }
    }

    #[test]
    fn default_codec_preserves_the_v1_envelope() {
        let registry = SnapshotRecordCodecRegistry::default();
        let encoded = registry.encode(&vec!["value"]).unwrap();

        assert_eq!(
            String::from_utf8(encoded.clone()).unwrap(),
            r#"{"format_version":1,"payload":["value"]}"#
        );
        assert_eq!(registry.decode::<Vec<String>>(&encoded).unwrap(), ["value"]);
    }

    #[test]
    fn bounded_decode_rejects_reader_output_over_the_resource_limit() {
        let registry = SnapshotRecordCodecRegistry::default();
        let encoded = registry.encode(&vec!["value"]).unwrap();

        let error = registry
            .decode_with_max_payload_bytes::<Vec<String>>(&encoded, 1)
            .unwrap_err();

        assert_eq!(error.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);
    }

    #[test]
    fn version_one_rejects_unknown_and_duplicate_envelope_fields() {
        let registry = SnapshotRecordCodecRegistry::default();

        let unknown = registry
            .decode::<serde_json::Value>(
                br#"{"format_version":1,"payload":null,"unexpected":true}"#,
            )
            .unwrap_err();
        let duplicate = registry
            .decode::<serde_json::Value>(
                br#"{"format_version":1,"format_version":1,"payload":null}"#,
            )
            .unwrap_err();

        assert_eq!(unknown.code.as_str(), ErrorCode::CORRUPT_STATE);
        assert_eq!(duplicate.code.as_str(), ErrorCode::CORRUPT_STATE);
    }

    #[test]
    fn additive_reader_handles_a_future_version_without_changing_the_writer() {
        let registry = SnapshotRecordCodecRegistry::default()
            .with_reader(Arc::new(JsonSnapshotRecordCodecV2))
            .unwrap();
        let v2 = JsonSnapshotRecordCodecV2
            .encode_payload(br#"{"value":42}"#)
            .unwrap();

        assert_eq!(registry.writer_version(), JSON_SNAPSHOT_RECORD_FORMAT_V1);
        assert_eq!(
            registry.decode::<serde_json::Value>(&v2).unwrap(),
            serde_json::json!({"value": 42})
        );
    }

    #[test]
    fn duplicate_versions_are_rejected_before_composition() {
        let error = SnapshotRecordCodecRegistry::default()
            .with_reader(Arc::new(JsonSnapshotRecordCodecV1))
            .unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::CONFLICT);
    }

    #[test]
    fn writer_version_is_the_immutable_registration_identity() {
        let registry =
            SnapshotRecordCodecRegistry::new(Arc::new(MutableVersionCodec::new(7, 7))).unwrap();

        assert_eq!(registry.writer_version(), 7);
        assert_eq!(registry.writer_version(), 7);
        assert_eq!(registry.readable_versions().collect::<Vec<_>>(), [7]);
        assert!(registry.encode(&"value").is_ok());
    }

    #[test]
    fn writer_output_cannot_escape_its_registered_version() {
        let registry =
            SnapshotRecordCodecRegistry::new(Arc::new(MutableVersionCodec::new(7, 8))).unwrap();

        let error = registry.encode(&"value").unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::INTERNAL);
        assert_eq!(
            error.message,
            "snapshot record codec declared version 7 but encoded version 8"
        );
    }

    #[test]
    fn codec_panics_are_contained_at_every_extension_boundary() {
        let registration =
            SnapshotRecordCodecRegistry::new(Arc::new(PanickingCodec(PanicPoint::Registration)))
                .unwrap_err();
        assert_eq!(registration.code.as_str(), ErrorCode::INTERNAL);

        let encoder =
            SnapshotRecordCodecRegistry::new(Arc::new(PanickingCodec(PanicPoint::Encode))).unwrap();
        let encoding = encoder.encode(&"value").unwrap_err();
        assert_eq!(encoding.code.as_str(), ErrorCode::INTERNAL);

        let decoder =
            SnapshotRecordCodecRegistry::new(Arc::new(PanickingCodec(PanicPoint::Decode))).unwrap();
        let decoding = decoder
            .decode::<String>(br#"{"format_version":9,"payload":"value"}"#)
            .unwrap_err();
        assert_eq!(decoding.code.as_str(), ErrorCode::INTERNAL);
    }
}
