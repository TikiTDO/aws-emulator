// SPDX-License-Identifier: CC0-1.0

//! Catalog envelopes reuse the generated model codecs. This is not a second
//! protocol implementation: model members and required-field rules stay owned by
//! SerializableStruct/DeserializableStruct and the existing Smithy JSON codec.
use aws_smithy_schema::{Schema, serde::{SerializableStruct, ShapeSerializer}};
use aws_smithy_schema_server::{DeserializableStruct, json::{JsonShapeSerializer, JsonShapeDeserializer}};
use service_common::payload_store::StoreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub trait Model: SerializableStruct + DeserializableStruct { fn schema() -> &'static Schema; }
macro_rules! models {
    ($($ty:ty),* $(,)?) => { $(impl Model for $ty { fn schema() -> &'static Schema { &<$ty>::SCHEMA } })* };
}
models!(
    crate::ops::types::ImageScanningConfiguration,
    crate::ops::types::EncryptionConfiguration,
    crate::ops::types::ImageTagMutabilityExclusionFilter,
    crate::ops::types::Tag,
    crate::ops::types::ImageScanStatus,
    crate::ops::types::ImageScanFinding,
    crate::ops::types::EncryptionConfigurationForRepositoryCreationTemplate,
    crate::ops::types::PullThroughCacheRule,
    crate::ops::types::ReplicationRule,
    crate::ops::types::SigningConfiguration,
    crate::ops::types::RegistryScanningConfiguration,
);
pub fn encode<T: Model>(value: &T) -> Result<Value, StoreError> {
    let mut serializer = JsonShapeSerializer::new();
    serializer.write_struct(T::schema(), value).map_err(|_| StoreError::Integrity)?;
    let text = serializer.into_string();
    if text.len() > 4 * 1024 * 1024 { return Err(StoreError::QuotaExceeded); }
    serde_json::from_str(&text).map_err(|_| StoreError::Integrity)
}
pub fn decode<T: Model>(value: &Value) -> Result<T, StoreError> {
    let bytes = serde_json::to_vec(value).map_err(|_| StoreError::Integrity)?;
    if bytes.len() > 4 * 1024 * 1024 { return Err(StoreError::Integrity); }
    T::deserialize(&mut JsonShapeDeserializer::new(&bytes)).map_err(|_| StoreError::Integrity)
}
pub mod model {
    use super::*;
    pub fn serialize<T: Model, S: serde::Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        encode(value).map_err(|_| serde::ser::Error::custom("catalog model encoding"))?.serialize(serializer)
    }
    pub fn deserialize<'de, T: Model, D: serde::Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
        decode(&Value::deserialize(deserializer)?).map_err(|_| serde::de::Error::custom("catalog model decoding"))
    }
}
pub mod model_vec {
    use super::*;
    pub fn serialize<T: Model, S: serde::Serializer>(values: &[T], serializer: S) -> Result<S::Ok, S::Error> {
        values.iter().map(encode).collect::<Result<Vec<_>, _>>().map_err(|_| serde::ser::Error::custom("catalog model list encoding"))?.serialize(serializer)
    }
    pub fn deserialize<'de, T: Model, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
        Vec::<Value>::deserialize(deserializer)?.iter().map(decode).collect::<Result<_, _>>().map_err(|_| serde::de::Error::custom("catalog model list decoding"))
    }
}
pub mod model_option {
    use super::*;
    pub fn serialize<T: Model, S: serde::Serializer>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_ref().map(encode).transpose().map_err(|_| serde::ser::Error::custom("catalog optional model encoding"))?.serialize(serializer)
    }
    pub fn deserialize<'de, T: Model, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
        Option::<Value>::deserialize(deserializer)?.as_ref().map(decode).transpose().map_err(|_| serde::de::Error::custom("catalog optional model decoding"))
    }
}
