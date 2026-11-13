// SPDX-License-Identifier: CC0-1.0

//! Registry-global control records in the same B2 authority as repositories.
//! Detached candidates contain no images, upload bytes, grants or credentials.
use super::*;
use super::{catalog_codec as codec, payloads};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use service_common::payload_store::{CommitError, PayloadStore, RecordChange, RecordCondition, RecordData, RecordKey, ReferenceTransaction, StageRetirement, StoreError};
use sha2::{Digest, Sha256};

type State = ComAmazonawsEcrAmazonec2containerregistryV20150921State;
const PREFIX: &str = "global/";
const DOCUMENT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Template {
    prefix: String, description: Option<String>,
    #[serde(with="super::catalog_codec::model")]
    encryption_config: EncryptionConfigurationForRepositoryCreationTemplate,
    #[serde(with="super::catalog_codec::model_vec")]
    resource_tags: Vec<Tag>,
    image_tag_mutability: String,
    #[serde(with="super::catalog_codec::model_vec")]
    image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>,
    repository_policy: Option<String>, lifecycle_policy: Option<String>,
    applied_for: Vec<String>, custom_role_arn: Option<String>, created_at: SystemTime, updated_at: SystemTime,
}
impl From<&CreationTemplate> for Template {
    fn from(v: &CreationTemplate) -> Self {
        Self { prefix:v.prefix.clone(), description:v.description.clone(), encryption_config:v.encryption_config.clone(),
            resource_tags:v.resource_tags.clone(), image_tag_mutability:v.image_tag_mutability.clone(),
            image_tag_mutability_exclusion_filters:v.image_tag_mutability_exclusion_filters.clone(),
            repository_policy:v.repository_policy.clone(), lifecycle_policy:v.lifecycle_policy.clone(), applied_for:v.applied_for.clone(),
            custom_role_arn:v.custom_role_arn.clone(), created_at:v.created_at, updated_at:v.updated_at }
    }
}
impl From<Template> for CreationTemplate {
    fn from(v: Template) -> Self {
        Self { prefix:v.prefix, description:v.description, encryption_config:v.encryption_config, resource_tags:v.resource_tags,
            image_tag_mutability:v.image_tag_mutability, image_tag_mutability_exclusion_filters:v.image_tag_mutability_exclusion_filters,
            repository_policy:v.repository_policy, lifecycle_policy:v.lifecycle_policy, applied_for:v.applied_for,
            custom_role_arn:v.custom_role_arn, created_at:v.created_at, updated_at:v.updated_at }
    }
}

/// Copy only the global control projection; all payload maps stay empty.
pub fn capture(state: &State) -> State {
    let mut result = State::default();
    result.pull_through_cache_rules = state.pull_through_cache_rules.clone();
    result.account_settings = state.account_settings.clone();
    result.creation_templates = state.creation_templates.clone();
    result.replication_rules = state.replication_rules.clone();
    result.registry_policy = state.registry_policy.clone();
    result.signing_configuration = state.signing_configuration.clone();
    result.pull_time_update_exclusions = state.pull_time_update_exclusions.clone();
    result.registry_scanning = state.registry_scanning.clone();
    result
}
pub fn snapshot<Eng: Send + Sync + 'static>(engine: &Eng) -> Result<State, StoreError> {
    let state = service_state(engine).ok_or(StoreError::Invalid("ECR state binding"))?;
    let result = capture(&state.lock());
    Ok(result)
}
pub(super) fn apply(state: &mut State, value: State) {
    state.pull_through_cache_rules = value.pull_through_cache_rules;
    state.account_settings = value.account_settings;
    state.creation_templates = value.creation_templates;
    state.replication_rules = value.replication_rules;
    state.registry_policy = value.registry_policy;
    state.signing_configuration = value.signing_configuration;
    state.pull_time_update_exclusions = value.pull_time_update_exclusions;
    state.registry_scanning = value.registry_scanning;
}
fn json<T: Serialize>(value: &T) -> Result<Value, StoreError> { serde_json::to_value(value).map_err(|_| StoreError::Integrity) }
fn names(state: &State) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    result.extend(state.account_settings.keys().map(|k| format!("setting/{k}")));
    result.extend(state.creation_templates.keys().map(|k| format!("template/{k}")));
    result.extend(state.pull_through_cache_rules.keys().map(|k| format!("rule/{k}")));
    result.extend(state.pull_time_update_exclusions.keys().map(|k| format!("exclusion/{k}")));
    if !state.replication_rules.is_empty() { result.insert("replication".to_string()); }
    if state.registry_policy.is_some() { result.insert("policy".to_string()); }
    if state.signing_configuration.is_some() { result.insert("signing".to_string()); }
    if state.registry_scanning.is_some() { result.insert("scanning".to_string()); }
    result
}
fn document(state: &State, name: &str) -> Result<Option<Bytes>, StoreError> {
    let value = if let Some(key) = name.strip_prefix("setting/") { state.account_settings.get(key).map(json).transpose()? }
    else if let Some(key) = name.strip_prefix("template/") { state.creation_templates.get(key).map(|v| json(&Template::from(v))).transpose()? }
    else if let Some(key) = name.strip_prefix("rule/") { state.pull_through_cache_rules.get(key).map(codec::encode).transpose()? }
    else if let Some(key) = name.strip_prefix("exclusion/") { state.pull_time_update_exclusions.get(key).map(json).transpose()? }
    else { match name {
        "replication" if !state.replication_rules.is_empty() => Some(Value::Array(state.replication_rules.iter().map(codec::encode).collect::<Result<_,_>>()?)),
        "policy" => state.registry_policy.as_ref().map(json).transpose()?,
        "signing" => state.signing_configuration.as_ref().map(codec::encode).transpose()?,
        "scanning" => state.registry_scanning.as_ref().map(codec::encode).transpose()?,
        _ => None,
    }};
    let Some(value) = value else { return Ok(None); };
    let bytes = serde_json::to_vec(&value).map_err(|_| StoreError::Integrity)?;
    if bytes.len() as u64 > DOCUMENT_LIMIT { return Err(StoreError::QuotaExceeded); }
    Ok(Some(Bytes::from(bytes)))
}
fn key(name: &str) -> Result<RecordKey, StoreError> {
    RecordKey::new(payloads::registry_owner()?, Bytes::from(format!("{PREFIX}{name}")))
}
fn validate_record(name: &str, value: &RecordData) -> Result<(), StoreError> {
    if value.schema_version != 1 || value.references.len() != 1
        || value.metadata.as_object().is_none_or(|v| v.len() != 2)
        || value.metadata.get("kind").and_then(Value::as_str) != Some("ecr-global")
        || value.metadata.get("key").and_then(Value::as_str) != Some(name) { return Err(StoreError::Integrity); }
    Ok(())
}

/// One conditional group, tracked by the existing engine-owned pending slot.
/// No live state changes before durable outcome; no await between ACK and apply.
pub async fn publish<Eng: Send + Sync + 'static>(engine: &Eng, mutation: &payloads::MutationGuard, candidate: State) -> Result<(), CommitError> {
    let store = payloads::bound_store(engine)?;
    let before = snapshot(engine)?;
    let keys: BTreeSet<_> = names(&before).into_iter().chain(names(&candidate)).collect();
    let mut changes = Vec::new(); let mut retire = Vec::new();
    for name in keys {
        let before_document = document(&before, &name)?;
        let after_document = document(&candidate, &name)?;
        if before_document == after_document { continue; }
        let key = key(&name)?;
        let previous = store.record(&key).await?;
        let old = previous.as_ref().and_then(|v| v.value.as_ref());
        match (before_document.as_ref(), old) {
            (None, None) => {},
            (Some(expected), Some(value)) => {
                validate_record(&name, value)?;
                let digest: [u8;32] = Sha256::digest(expected).into();
                if value.references[0].len() != expected.len() as u64 || value.references[0].sha256().as_bytes() != &digest {
                    return Err(StoreError::CheckpointConflict.into());
                }
            },
            _ => return Err(StoreError::CheckpointConflict.into()),
        }
        let value = if let Some(bytes) = after_document.as_ref() {
            let (sealed, checkpoint) = payloads::stage_document(&store, payloads::registry_owner()?, bytes.clone(), DOCUMENT_LIMIT).await?;
            retire.push(StageRetirement { upload:sealed.upload().clone(), checkpoint });
            Some(RecordData { schema_version:1, metadata:serde_json::json!({"kind":"ecr-global","key":name}), references:vec![sealed.handle().clone()] })
        } else { None };
        changes.push(RecordChange { key, condition:previous.filter(|v| v.value.is_some()).map_or(RecordCondition::Absent, |v| RecordCondition::Exact(v.revision)), value });
    }
    if !changes.is_empty() {
        mutation.commit(&store, ReferenceTransaction { id:store.new_commit_id(), owner:payloads::registry_owner()?, changes, retire }).await?;
    }
    let state = service_state(engine).ok_or(StoreError::Invalid("ECR state binding"))?;
    apply(&mut state.lock(), candidate);
    Ok(())
}

/// Source-owned loader; caller keeps the mutation token through private assembly
/// and final application alongside repository state. No global-nonempty shortcut.
pub(super) async fn load(store: &PayloadStore, _guard: &payloads::MutationGuard) -> Result<(State, usize), StoreError> {
    let mut state = State::default(); let mut count = 0; let mut cursor = None;
    loop {
        let page = store.list_records(&payloads::registry_owner()?, cursor, 1).await?;
        for record in page.records {
            let Some(value) = record.value else { continue; };
            let name = std::str::from_utf8(record.key.key()).map_err(|_| StoreError::Integrity)?;
            let Some(name) = name.strip_prefix(PREFIX) else { continue; };
            validate_record(name, &value)?;
            let text = payloads::read_document(store, &value.references[0]).await?;
            let document: Value = serde_json::from_str(&text).map_err(|_| StoreError::Integrity)?;
            decode(&mut state, name, document)?;
            count += 1;
        }
        cursor = page.next;
        if cursor.is_none() { break; }
    }
    // Re-encoding validates the same per-document bound used by writers.
    for name in names(&state) { document(&state, &name)?; }
    Ok((state,count))
}
fn from_json<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, StoreError> { serde_json::from_value(value).map_err(|_| StoreError::Integrity) }
fn decode(state: &mut State, name: &str, value: Value) -> Result<(), StoreError> {
    if let Some(key) = name.strip_prefix("setting/") {
        let value: String = from_json(value)?;
        state.set_account_setting(key, &value).map_err(|_| StoreError::Integrity)?;
    } else if let Some(key) = name.strip_prefix("template/") {
        let template: Template = from_json(value)?;
        if normalize_prefix(&template.prefix) != key || state.creation_templates.len() >= MAX_REPOSITORY_CREATION_TEMPLATES { return Err(StoreError::Integrity); }
        state.creation_templates.insert(key.to_string(), template.into());
    } else if let Some(key) = name.strip_prefix("rule/") {
        let rule: PullThroughCacheRule = codec::decode(&value)?;
        if rule.ecr_repository_prefix.as_ref().is_none_or(|v| normalize_prefix(v) != key)
            || state.pull_through_cache_rules.len() >= MAX_PULL_THROUGH_CACHE_RULES { return Err(StoreError::Integrity); }
        state.pull_through_cache_rules.insert(key.to_string(), rule);
    } else if let Some(key) = name.strip_prefix("exclusion/") {
        if key.is_empty() || key.len() > 2048 || state.pull_time_update_exclusions.len() >= MAX_PULL_TIME_UPDATE_EXCLUSIONS { return Err(StoreError::Integrity); }
        state.pull_time_update_exclusions.insert(key.to_string(), from_json(value)?);
    } else { match name {
        "replication" => { state.replication_rules = value.as_array().ok_or(StoreError::Integrity)?.iter().map(codec::decode).collect::<Result<_,_>>()?; },
        "policy" => { let policy: String = from_json(value)?; validate_registry_policy(&policy).map_err(|_| StoreError::Integrity)?; state.registry_policy = Some(policy); },
        "signing" => state.signing_configuration = Some(codec::decode(&value)?),
        "scanning" => state.registry_scanning = Some(codec::decode(&value)?),
        _ => return Err(StoreError::Integrity),
    }}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_projection_contains_no_payload_or_credentials() {
        let mut state = State::default(); state.account_settings.insert("x".into(),"y".into());
        let candidate = capture(&state);
        assert!(candidate.repositories.is_empty() && candidate.uploads.is_empty() && candidate.registry_credentials.is_empty());
        assert_eq!(candidate.account_settings, state.account_settings);
    }
    #[test]
    fn global_namespace_is_closed() {
        assert!(decode(&mut State::default(), "unknown", Value::Null).is_err());
        assert!(decode(&mut State::default(), "template/root", serde_json::json!({})).is_err());
    }
    #[test]
    fn template_roundtrip_uses_the_real_generated_model_codec() {
        let mut source = State::default();
        source.creation_templates.insert("team".into(), CreationTemplate {
            prefix:"team".into(), description:Some("fixture".into()), encryption_config:default_template_encryption(),
            resource_tags:Vec::new(), image_tag_mutability:"MUTABLE".into(), image_tag_mutability_exclusion_filters:Vec::new(),
            repository_policy:None, lifecycle_policy:None, applied_for:vec!["PULL_THROUGH_CACHE".into()],
            custom_role_arn:None, created_at:std::time::UNIX_EPOCH, updated_at:std::time::UNIX_EPOCH,
        });
        let encoded = document(&source,"template/team").unwrap().unwrap();
        let mut restored = State::default();
        decode(&mut restored,"template/team",serde_json::from_slice(&encoded).unwrap()).unwrap();
        assert_eq!(document(&restored,"template/team").unwrap(),Some(encoded));
        assert!(restored.repositories.is_empty());
    }
}
