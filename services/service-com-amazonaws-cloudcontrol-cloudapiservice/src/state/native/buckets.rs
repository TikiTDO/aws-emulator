// SPDX-License-Identifier: CC0-1.0

//! `AWS::S3::Bucket`, as it lives in the Amazon S3 service's own store.
//!
//! A bucket is a resource of the Amazon S3 service: `s3:CreateBucket` creates it,
//! `s3:HeadBucket` and `s3:ListBuckets` report it, `s3:PutBucketTagging` and
//! `s3:PutBucketVersioning` change it and `s3:DeleteBucket` removes it, and the record of
//! it lives in that service's state in the same shared engine. So this provider holds a
//! handle to THAT store and nothing of its own: a bucket this emulator holds is one Cloud
//! Control API resolves whichever API created it, a bucket Amazon S3 no longer holds is
//! one Cloud Control API reports as not found, and a bucket a `CreateResource` provisioned
//! is one the S3 API itself serves objects in — which is the whole point of provisioning
//! it through a uniform control plane.
//!
//! A bucket's name is "unique across all AWS accounts in all the AWS Regions within a
//! partition" (Amazon S3 User Guide, *General purpose bucket naming rules*), so the owning
//! service holds its buckets in ONE namespace and a reference resolves to the same bucket
//! whichever Region the call naming it was signed for; the Region a create was signed for
//! is the Region the bucket is then placed in.
//!
//! Only the buckets of the caller's own S3 account are resources of this type: a bucket
//! another emulated service provisioned as the object store of one of ITS resources, and a
//! bucket AWS publishes, are not the account's — which is exactly the distinction that
//! service's own `ListBuckets` reader draws
//! (`s3::ComAmazonawsS3Amazons3State::general_purpose_account_buckets`), so this provider
//! resolves and enumerates through it.
//!
//! The resource model is the type's registry schema. Catalog source: the `AWS::S3::Bucket`
//! resource provider schema's `properties`, whose `primaryIdentifier` is
//! `/properties/BucketName`. The properties this path APPLIES are the identity, the
//! bucket's `Tags` and its `VersioningConfiguration`, each carried by the owning record and
//! each read and written through that service's own rules; the read-only `Arn` is reported
//! beside them.
//!
//! Every other property the schema declares ([`UNAPPLIED_PROPERTIES`]) is one THIS path
//! cannot apply, so a model that asks for one is refused rather than accepted and dropped —
//! a property a caller sets has to be one a later read reports. Each of them is a
//! subresource the owning service installs with a call of its own (`PutBucketEncryption`,
//! `PutBucketLifecycleConfiguration`, `PutPublicAccessBlock`, …) rather than a field of the
//! bucket's record, or a value that service derives rather than stores.

use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_s3_amazons3::ops::types::BUCKET_VERSIONING_STATUS_VALUES;
use ::service_com_amazonaws_s3_amazons3::state as s3;

use super::{require_value, text, NativeRefusal, NativeResourceProvider, S3State};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::S3::Bucket";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/BucketName`): the bucket's name, which the
/// caller chooses.
const IDENTIFIER_PROPERTY: &str = "BucketName";

/// The read-only property of the type a read reports beside the identity: the bucket's ARN
/// (one of the schema's `readOnlyProperties`), built by the owning service's own ARN
/// builder.
const ARN: &str = "Arn";

/// The bucket's tag set (`Tags`), a list of `{ Key, Value }` pairs.
const TAGS: &str = "Tags";

/// The key member of one entry of [`TAGS`].
const TAG_KEY: &str = "Key";

/// The value member of one entry of [`TAGS`].
const TAG_VALUE: &str = "Value";

/// The bucket's versioning state (`VersioningConfiguration`), the container
/// `s3:PutBucketVersioning` carries.
const VERSIONING_CONFIGURATION: &str = "VersioningConfiguration";

/// The one member of [`VERSIONING_CONFIGURATION`] this path applies: the versioning
/// `Status`.
const VERSIONING_STATUS: &str = "Status";

/// The properties of the type's schema THIS path does not apply — see this module's header.
///
/// Named here so a model asking for one is told it is a property of the type this handler
/// does not apply, rather than one the type does not declare at all: the two are different
/// faults and a caller fixes them differently.
const UNAPPLIED_PROPERTIES: &[&str] = &[
    "AccelerateConfiguration",
    "AccessControl",
    "AnalyticsConfigurations",
    "BucketEncryption",
    "CorsConfiguration",
    "DomainName",
    "DualStackDomainName",
    "IntelligentTieringConfigurations",
    "InventoryConfigurations",
    "LifecycleConfiguration",
    "LoggingConfiguration",
    "MetadataTableConfiguration",
    "MetricsConfigurations",
    "NotificationConfiguration",
    "ObjectLockConfiguration",
    "ObjectLockEnabled",
    "OwnershipControls",
    "PublicAccessBlockConfiguration",
    "RegionalDomainName",
    "ReplicationConfiguration",
    "WebsiteConfiguration",
    "WebsiteURL",
];

/// The Amazon S3 buckets of one engine, as Cloud Control API resources.
pub struct BucketProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<S3State>,
}

impl BucketProvider {
    /// The provider for the Amazon S3 store THIS engine carries, or `None` when it carries
    /// none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        s3::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for BucketProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        // One namespace, whichever Region the call was signed for: see this module's
        // header.
        let _ = region;
        let store = self.state.lock();
        account_bucket(&store, identifier).map(|bucket| model(identifier, bucket))
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let _ = region;
        let store = self.state.lock();
        // The owning service's own reader of "the buckets in your account" answers in
        // bucket-name order, so the window a listing pages over is that order resumed
        // after `after` — nothing is sorted here.
        store
            .general_purpose_account_buckets()
            .filter(|(name, _)| after.is_none_or(|after| *name > after))
            .take(limit)
            .map(|(name, bucket)| (name.to_string(), model(name, bucket)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // A bucket's record carries no instant a resource model moves.
        let _ = at;
        // The name a bucket is created under is the OWNING service's to accept: the
        // published general purpose bucket naming rules, read through the one shared
        // reading of them this workspace keeps, so nothing is created here under a name
        // that service's own API would refuse.
        if !::service_common::s3_bucket_name::is_well_formed(identifier) {
            return Err(NativeRefusal::Invalid(
                s3::INVALID_BUCKET_NAME_MESSAGE.to_string(),
            ));
        }
        // Read against what a bucket this create is about to make HOLDS: its own ARN, and
        // no tag set yet. The ARN comes from the owning store's own builder for a general
        // purpose bucket — the one the record's `arn()` answers with once the bucket is
        // there ([`model`]) — because the model has to be judged before anything is
        // written, so there is no record to read it off yet.
        let desired = DesiredModel::read(identifier, properties, &s3::bucket_arn(identifier), &[])?;
        let mut store = self.state.lock();
        match store.create_bucket(identifier, region) {
            // A name the owning store already holds is that service's own taken-name
            // verdict, whoever holds the bucket under it — one bucket namespace.
            s3::BucketCreation::NameTaken => return Err(NativeRefusal::AlreadyExists),
            // Its account quota bounds what this create may add.
            s3::BucketCreation::QuotaReached => {
                return Err(NativeRefusal::AtCapacity(format!(
                    "The account already holds the maximum of {} buckets",
                    s3::MAX_BUCKETS_PER_ACCOUNT
                )));
            }
            s3::BucketCreation::Created => {}
        }
        let Some(bucket) = store.bucket_mut(identifier) else {
            // Unreachable: the create above recorded it, under this same guard.
            return Err(NativeRefusal::NotFound);
        };
        desired.apply(bucket);
        // The subresources a create installs come into being WITH the bucket rather than
        // being written onto it afterwards, which is what the owning record's own creation
        // bookkeeping says.
        bucket.forget_configuration_writes();
        Ok(model(identifier, bucket))
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // One namespace, and a bucket's record carries no instant a resource model moves.
        let _ = (region, at);
        let mut store = self.state.lock();
        // A bucket that is not the account's own is not a resource of this type, so there
        // is none of this name to patch.
        let held = account_bucket(&store, identifier).ok_or(NativeRefusal::NotFound)?;
        let (arn, in_force) = (held.arn(identifier), held.tags().to_vec());
        let desired = DesiredModel::read(identifier, properties, &arn, &in_force)?;
        let bucket = store
            .bucket_mut(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        desired.apply(bucket);
        Ok(model(identifier, bucket))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let _ = region;
        let mut store = self.state.lock();
        let held = account_bucket(&store, identifier).ok_or(NativeRefusal::NotFound)?;
        // "All objects (including all object versions and delete markers) in the bucket
        // must be deleted before the bucket itself can be deleted" — the owning service
        // raises its own error about the bucket as it is, which the handler maps to no
        // verdict of its own, and the bucket stays.
        if !held.holds_no_versions() {
            let message = if held.versioning.is_some() {
                s3::BUCKET_NOT_EMPTY_VERSIONED_MESSAGE
            } else {
                s3::BUCKET_NOT_EMPTY_MESSAGE
            };
            return Err(NativeRefusal::Rejected(message.to_string()));
        }
        store.remove_bucket(identifier, at);
        Ok(())
    }
}

/// The bucket of the CALLER'S OWN S3 account named `identifier`, or `None` when that
/// service holds no such bucket of the account.
///
/// The one resolution this provider reads a bucket through, so a read, a patch and a delete
/// cannot disagree about which buckets are resources of this type.
fn account_bucket<'a>(store: &'a S3State, identifier: &str) -> Option<&'a s3::Bucket> {
    store
        .general_purpose_account_buckets()
        .find(|(name, _)| *name == identifier)
        .map(|(_, bucket)| bucket)
}

/// The resource model one bucket renders as — the schema's properties the owning record
/// carries, and only those it HAS: a bucket with no tag set and no versioning state carries
/// neither property, the same way the owning service's own readers report neither.
fn model(name: &str, bucket: &s3::Bucket) -> Value {
    let mut model = Map::new();
    model.insert(IDENTIFIER_PROPERTY.to_string(), Value::from(name));
    model.insert(ARN.to_string(), Value::from(bucket.arn(name)));
    if !bucket.tags().is_empty() {
        let tags: Vec<Value> = bucket
            .tags()
            .iter()
            .map(|tag| {
                Value::Object(Map::from_iter([
                    (TAG_KEY.to_string(), Value::from(tag.key.clone())),
                    (TAG_VALUE.to_string(), Value::from(tag.value.clone())),
                ]))
            })
            .collect();
        model.insert(TAGS.to_string(), Value::Array(tags));
    }
    if let Some(versioning) = &bucket.versioning {
        model.insert(
            VERSIONING_CONFIGURATION.to_string(),
            Value::Object(Map::from_iter([(
                VERSIONING_STATUS.to_string(),
                Value::from(versioning.status.clone()),
            )])),
        );
    }
    Value::Object(model)
}

/// What a resource model asks a bucket to be: the properties this path APPLIES, having been
/// checked against the owning service's own rules.
struct DesiredModel {
    /// The tag set the bucket is to carry.
    tags: Vec<s3::ResourceTag>,
    /// The versioning status the bucket is to be in, or `None` to leave it in the one it is
    /// in — which, for a bucket no configuration has been put on, is no versioning state at
    /// all.
    versioning_status: Option<String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the bucket `identifier`, whose own ARN is `arn` and
    /// whose tag set is `in_force` — or state why the owning service would not take it.
    ///
    /// The identity and the read-only ARN may be CARRIED (a read reports them, so a model a
    /// client round-trips carries them) but only with the value the bucket already answers
    /// to: a value accepted here and never applied would be one no later read reports.
    fn read(
        identifier: &str,
        properties: &Value,
        arn: &str,
        in_force: &[s3::ResourceTag],
    ) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            tags: Vec::new(),
            versioning_status: None,
        };
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, identifier)?,
                ARN => require_value(property, value, arn)?,
                TAGS => desired.tags = tag_set(value)?,
                VERSIONING_CONFIGURATION => {
                    desired.versioning_status = Some(versioning_status(value)?);
                }
                other if UNAPPLIED_PROPERTIES.contains(&other) => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is a property of {TYPE_NAME} this emulator's handler does \
                         not apply"
                    )));
                }
                other => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )));
                }
            }
        }
        // The tag set the model leaves the bucket with is judged by the owning service's OWN
        // rule for a bucket's tag set, HERE rather than where the property is read, so the
        // set that rule sees is the one about to be stored whether the model carried the
        // property or left it out: a model naming no `Tags` asks for the empty set, which is
        // a tag-set write like any other. `in_force` is the set it replaces, which that rule
        // reads — a key in the namespace AWS keeps for its own tags may be stated back
        // unchanged but neither added nor updated by a requester, and a Cloud Control API
        // call is a requester's.
        s3::validate_tag_set(
            &desired.tags,
            s3::BUCKET_TAG_SET_LIMITS,
            s3::TagKeyOrigin::Caller,
            in_force,
        )
        .map_err(|rejection| NativeRefusal::Invalid(rejection.to_string()))?;
        Ok(desired)
    }

    /// Write this model onto `bucket`. A property the model does not carry is CLEARED where
    /// the owning service publishes a way to clear it — a tag set is replaced, so a model
    /// naming none leaves the bucket untagged — and left alone where it publishes none: the
    /// API has no call that takes a bucket back to having no versioning state at all, so a
    /// model naming no `VersioningConfiguration` asks for nothing.
    fn apply(self, bucket: &mut s3::Bucket) {
        bucket.replace_tags(self.tags);
        if let Some(status) = self.versioning_status {
            bucket.set_versioning(&status, None);
        }
    }
}

/// The versioning status the `VersioningConfiguration` `value` names, or why the owning
/// service would not apply it.
///
/// "You can set the versioning state with one of the following values: Enabled … Suspended"
/// (`PutBucketVersioning`), which is the pair that service's own model declares
/// ([`BUCKET_VERSIONING_STATUS_VALUES`]); a container naming no status, or one outside that
/// pair, is a configuration it cannot apply.
fn versioning_status(value: &Value) -> Result<String, NativeRefusal> {
    let status = value
        .get(VERSIONING_STATUS)
        .map(|status| text(VERSIONING_STATUS, status))
        .transpose()?
        .filter(|status| BUCKET_VERSIONING_STATUS_VALUES.contains(&status.as_str()))
        .ok_or_else(|| {
            NativeRefusal::Invalid(format!(
                "{VERSIONING_CONFIGURATION}.{VERSIONING_STATUS} must be one of \
                 {BUCKET_VERSIONING_STATUS_VALUES:?}"
            ))
        })?;
    Ok(status)
}

/// The tag set `value` carries: a list of `{ Key, Value }` pairs.
///
/// What the owning service's OWN rule makes of the set is asked once the whole model has
/// been read ([`DesiredModel::read`]), so the set that rule judges is the one about to be
/// stored whether the model carried this property or left it out.
fn tag_set(value: &Value) -> Result<Vec<s3::ResourceTag>, NativeRefusal> {
    let Some(entries) = value.as_array() else {
        return Err(NativeRefusal::Invalid(format!(
            "{TAGS} must be a list of {{ {TAG_KEY}, {TAG_VALUE} }} pairs"
        )));
    };
    let mut tags = Vec::with_capacity(entries.len());
    for entry in entries {
        let key = entry.get(TAG_KEY).and_then(Value::as_str);
        let tag_value = entry.get(TAG_VALUE).and_then(Value::as_str);
        let (Some(key), Some(tag_value)) = (key, tag_value) else {
            return Err(NativeRefusal::Invalid(format!(
                "every {TAGS} entry must carry {TAG_KEY} and {TAG_VALUE}"
            )));
        };
        tags.push(s3::ResourceTag {
            key: key.to_string(),
            value: tag_value.to_string(),
        });
    }
    Ok(tags)
}
