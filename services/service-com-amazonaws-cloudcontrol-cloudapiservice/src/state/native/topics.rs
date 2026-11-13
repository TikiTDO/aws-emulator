// SPDX-License-Identifier: CC0-1.0

//! `AWS::SNS::Topic`, as it lives in the Amazon SNS service's own store.
//!
//! A topic is a resource of the Amazon SNS service: `sns:CreateTopic` creates it,
//! `sns:GetTopicAttributes` and `sns:ListTopics` report it, `sns:SetTopicAttributes`
//! changes it and `sns:DeleteTopic` removes it, and the record of it lives in that
//! service's state in the same shared engine. So this provider holds a handle to THAT
//! store and nothing of its own: a topic this emulator holds is one Cloud Control API
//! resolves, whichever API created it, and a topic Amazon SNS no longer holds is one Cloud
//! Control API reports as not found.
//!
//! Topics are regional — a topic lives in the Region its own ARN carries
//! ([`sns::belongs_to_region`]) — so every answer here is about the Region the call was
//! made in.
//!
//! The resource model is the type's registry schema. Catalog source: the `AWS::SNS::Topic`
//! resource provider schema's `properties`, whose `primaryIdentifier` is
//! `/properties/TopicArn` — the ARN Amazon SNS assigns, which is therefore also one of its
//! `readOnlyProperties` — and whose `additionalIdentifiers` are `/properties/TopicName`
//! (the two references [`topic_arn_of`] resolves):
//!
//! * `TopicName`, the topic's name, which its ARN is built from;
//! * every property that IS one of the topic's own attributes, named identically on both
//!   sides (`DisplayName`, `KmsMasterKeyId`, `SignatureVersion`, `TracingConfig`,
//!   `ArchivePolicy`, `ContentBasedDeduplication`, `FifoTopic`, `FifoThroughputScope`) —
//!   [`APPLIED_PROPERTIES`];
//! * `Tags`, the topic's tags, as a list of `{ Key, Value }` pairs;
//! * the read-only `TopicArn`.
//!
//! A property is carried in the form the schema declares — a flag as a boolean, an archive
//! policy as an object — while an attribute VALUE is text in the owning store, so the two
//! are converted here ([`PropertyForm`]) and every rule about what an attribute accepts
//! stays the owning service's own ([`sns::topic_attributes::topic_attribute_refusal`]).
//!
//! The schema's remaining properties are ones THIS path cannot apply, so a model that asks
//! for one is refused rather than accepted and dropped — a property a caller sets has to be
//! one a later read reports:
//!
//! * `Subscription`, because a subscription is a resource of its own that the owning
//!   service's `Subscribe` creates (and `AWS::SNS::Subscription` describes);
//! * `DataProtectionPolicy` and `DeliveryStatusLogging`, because the owning service keeps
//!   each outside the topic's attribute map — a document a call of its own installs
//!   (`PutDataProtectionPolicy`), and the per-protocol feedback attributes a role is
//!   configured through.
//!
//! An attribute of the topic this type's schema does NOT declare — its `Policy`, its
//! `DeliveryPolicy`, carried by resource types of their own — is none of this type's
//! business either way: a read does not report it, and a write of this type leaves it
//! exactly as the owning store holds it.

use ::std::collections::{BTreeMap, HashMap};
use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_sns_amazonsimplenotificationservice::state as sns;

use super::{
    generated_name, require_value, tag_list, tag_set, text, unsupported, NativeRefusal,
    NativeResourceProvider, SnsState,
};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::SNS::Topic";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/TopicArn`): the topic's ARN, which Amazon
/// SNS assigns rather than the caller naming it.
const IDENTIFIER_PROPERTY: &str = "TopicArn";

/// The topic's name (`TopicName`), which the schema declares create-only and which its ARN
/// is built from.
const TOPIC_NAME: &str = "TopicName";

/// Whether the topic is a FIFO one (`FifoTopic`), which its name and its applicable
/// attributes both follow from.
const FIFO_TOPIC: &str = "FifoTopic";

/// The subscriptions a template may declare inline (`Subscription`), each of which is a
/// resource of its own; applied by nothing here (see this module's header).
const SUBSCRIPTION: &str = "Subscription";

/// The data protection policy the owning service keeps on the topic
/// (`DataProtectionPolicy`), installed by a call of its own and applied by nothing here.
const DATA_PROTECTION_POLICY: &str = "DataProtectionPolicy";

/// The delivery-status logging configuration (`DeliveryStatusLogging`), which the owning
/// service carries as its per-protocol feedback attributes and nothing here applies.
const DELIVERY_STATUS_LOGGING: &str = "DeliveryStatusLogging";

/// The topic's tag set (`Tags`), a list of `{ Key, Value }` pairs — read and rendered by
/// the shared reading of a tag set every provider uses.
const TAGS: &str = "Tags";

/// How the schema declares a property whose value the owning store holds as attribute
/// TEXT, which is what a model carries it as and what a read reports it as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyForm {
    /// A string, held as itself.
    Text,
    /// A boolean, held as `true`/`false` — the text the owning service reads a flag
    /// attribute from ([`sns::flag_attribute_enabled`]).
    Flag,
    /// A JSON object, held as the document's own text.
    Document,
}

impl PropertyForm {
    /// The attribute text `value` carries for a property of this form, or why it is not a
    /// value of that form at all.
    fn attribute_text(self, property: &str, value: &Value) -> Result<String, NativeRefusal> {
        match self {
            Self::Text => text(property, value),
            Self::Flag => value.as_bool().map(|flag| flag.to_string()).ok_or_else(|| {
                NativeRefusal::Invalid(format!("{property} must be true or false"))
            }),
            Self::Document => value
                .as_object()
                .map(|_| value.to_string())
                .ok_or_else(|| NativeRefusal::Invalid(format!("{property} must be a document"))),
        }
    }

    /// The attribute text `held` as the PROPERTY of this form reports it. A value the
    /// owning store somehow holds in another form is reported as the text it is rather
    /// than dropped from the model.
    fn property_value(self, held: &str) -> Value {
        let as_text = || Value::from(held.to_string());
        match self {
            Self::Text => as_text(),
            Self::Flag => held.parse::<bool>().map_or_else(|_| as_text(), Value::from),
            Self::Document => ::serde_json::from_str(held).unwrap_or_else(|_| as_text()),
        }
    }
}

/// The properties of this type that ARE one of the topic's own attributes under the same
/// name, with the form the schema declares each in.
///
/// Catalog source: the `AWS::SNS::Topic` schema's `properties` intersected with the
/// attribute names the owning service defines for a topic (`sns::TOPIC_ATTRIBUTE_NAMES`),
/// which this module's own test pins.
const APPLIED_PROPERTIES: &[(&str, PropertyForm)] = &[
    ("ArchivePolicy", PropertyForm::Document),
    ("ContentBasedDeduplication", PropertyForm::Flag),
    ("DisplayName", PropertyForm::Text),
    ("FifoThroughputScope", PropertyForm::Text),
    (FIFO_TOPIC, PropertyForm::Flag),
    ("KmsMasterKeyId", PropertyForm::Text),
    ("SignatureVersion", PropertyForm::Text),
    ("TracingConfig", PropertyForm::Text),
];

/// The form `property` is declared in when it is one of [`APPLIED_PROPERTIES`], and `None`
/// for anything else — a property of the type this path does not apply, and an attribute of
/// the topic this type declares no property for.
fn applied_property(property: &str) -> Option<PropertyForm> {
    APPLIED_PROPERTIES
        .iter()
        .find(|(name, _)| *name == property)
        .map(|(_, form)| *form)
}

/// The Amazon SNS topics of one engine, as Cloud Control API resources.
pub struct TopicProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<SnsState>,
}

impl TopicProvider {
    /// The provider for the Amazon SNS store THIS engine carries, or `None` when it carries
    /// none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        sns::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for TopicProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    /// The ARN the topic a create describes will answer to.
    ///
    /// The type's primary identifier is read-only in its schema, so no desired state
    /// carries it: the topic's ARN is the owning service's to assign, and it assigns the
    /// one built from the Region, this account and the topic's name. A model that names no
    /// name is a resource whose name the provider picks ([`generated_name`]), the same way
    /// a template that declares none leaves the name to CloudFormation.
    fn assigned_identifier(&self, region: &str, properties: &Value) -> Option<String> {
        let name = properties
            .get(TOPIC_NAME)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                generated_name(designates_fifo(properties.get(FIFO_TOPIC)), sns::FIFO_SUFFIX)
            });
        Some(sns::topic_arn(region, &name))
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        let arn = topic_arn_of(region, identifier)?;
        self.state.lock().topics.get(&arn).map(model)
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        // The owning store keys its topics by ARN in a hash map, so the ARN order a listing
        // pages over is built here — over the records of THIS Region that sort after the
        // position the page resumes at, which is bounded by that store's own record ceiling.
        let store = self.state.lock();
        let mut found: Vec<&sns::TopicRecord> = store
            .topics
            .values()
            .filter(|topic| sns::belongs_to_region(&topic.arn, region))
            .filter(|topic| after.is_none_or(|after| topic.arn.as_str() > after))
            .collect();
        found.sort_by(|left, right| left.arn.cmp(&right.arn));
        found
            .into_iter()
            .take(limit)
            .map(|topic| (topic.arn.clone(), model(topic)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let Some(arn) = topic_arn_of(region, identifier) else {
            return Err(NativeRefusal::Invalid(format!(
                "{IDENTIFIER_PROPERTY} {}",
                topic_arn_requirement()
            )));
        };
        let name = sns::topic_name_from_arn(&arn)
            .ok_or_else(|| {
                NativeRefusal::Invalid(format!(
                    "{IDENTIFIER_PROPERTY} {}",
                    topic_arn_requirement()
                ))
            })?
            .to_string();
        let desired = DesiredModel::read(&name, &arn, properties)?;
        let fifo = desired.designates_fifo();
        // The name a topic is created under is the OWNING service's to accept, and the name
        // and the topic's KIND constrain each other there — so nothing is created here
        // under a name that service's own API would refuse.
        if !sns::create_topic_name_is_valid(&name, fifo) {
            return Err(NativeRefusal::Invalid(sns::INVALID_TOPIC_NAME.to_string()));
        }
        let attributes = desired.attributes(fifo, &[])?;
        let tags = desired.tag_map();
        tags_accepted(&tags)?;
        record_fits(&attributes)?;

        let mut store = self.state.lock();
        // The topic is created in the owning service's store under its own rules: an ARN it
        // already holds is that service's own already-exists verdict, and its record
        // ceiling bounds what this create may add.
        if store.topics.contains_key(&arn) {
            return Err(NativeRefusal::AlreadyExists);
        }
        if store.topics.len() >= sns::MAX_RECORDS_PER_STORE {
            return Err(NativeRefusal::AtCapacity(sns::limit_exceeded_message(
                "topics",
            )));
        }
        let mut topic = sns::TopicRecord {
            arn: arn.clone(),
            name,
            owner: sns::owner_account_id(),
            attributes,
            tags,
            // A document the owning service installs with a call of its own, which this
            // create does not make (see this module's header).
            data_protection_policy: None,
            is_fifo: fifo,
            created_at: at,
            subscriptions_deleted: 0,
            // A topic nothing has subscribed to has no deletion to take up either.
            subscriptions_deleting: Vec::new(),
            archive_enabled_at: None,
        };
        // A topic created WITH an archive policy archives from the moment it exists, which
        // is the owning record's own rule about its archive window.
        topic.refresh_archive_window();
        let created = model(&topic);
        store.topics.insert(arn, topic);
        Ok(created)
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // A topic's record carries no instant a model changes: what the write leaves is the
        // attribute map and the tag set.
        let _ = at;
        let arn = topic_arn_of(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        let held = store.topics.get(&arn).ok_or(NativeRefusal::NotFound)?;
        let (name, was_fifo) = (held.name.clone(), held.is_fifo);
        let held_attributes = held.attributes.clone();
        let desired = DesiredModel::read(&name, &arn, properties)?;
        // "You can provide this attribute only during topic creation": the topic's kind is
        // settled, so a model that designates the other one asks the owning service for a
        // change it does not make.
        if desired.designates_fifo() != was_fifo {
            return Err(NativeRefusal::Invalid(format!(
                "{FIFO_TOPIC} of this resource is {was_fifo}"
            )));
        }
        // An attribute of the topic this type's schema does not declare is none of this
        // type's business (see this module's header), so a write of THIS type leaves it
        // exactly as the owning store holds it rather than clearing it with a model that
        // could never have carried it.
        let protocols: Vec<String> = store
            .subscriptions_of(&arn)
            .into_iter()
            .map(|subscription| subscription.protocol.clone())
            .collect();
        let mut attributes = desired.attributes(was_fifo, &protocols)?;
        for (attribute, value) in &held_attributes {
            if applied_property(attribute).is_none() {
                attributes.insert(attribute.clone(), value.clone());
            }
        }
        let tags = desired.tag_map();
        tags_accepted(&tags)?;
        record_fits(&attributes)?;

        let topic = store.topics.get_mut(&arn).ok_or(NativeRefusal::NotFound)?;
        topic.attributes = attributes;
        topic.tags = tags;
        // The archive window follows the attribute map: a write that adds the archive
        // policy starts archiving now, and one that removes it ends the window.
        topic.refresh_archive_window();
        Ok(model(topic))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let _ = at;
        let arn = topic_arn_of(region, identifier).ok_or(NativeRefusal::NotFound)?;
        let mut store = self.state.lock();
        if store.topics.remove(&arn).is_none() {
            return Err(NativeRefusal::NotFound);
        }
        // Removed through the owning service's own removal: deleting a topic deletes every
        // subscription to it, the messages delivered to those subscriptions go with them,
        // and the topic's message archive goes with the topic.
        let orphaned: Vec<String> = store
            .subscriptions_of(&arn)
            .into_iter()
            .map(|subscription| subscription.arn.clone())
            .collect();
        for subscription in orphaned {
            store.subscriptions.remove(&subscription);
            store.forget_deliveries(&subscription);
        }
        store.forget_archive(&arn);
        Ok(())
    }
}

/// The ARN of the topic `identifier` names in `region`, or `None` for a value that names no
/// topic of this type at all.
///
/// The references a resource of this type answers to are the ones the type's schema
/// DECLARES: its `primaryIdentifier` `/properties/TopicArn` — the ARN Amazon SNS assigns,
/// built from the Region, this emulator's account and the topic's name — and its
/// `additionalIdentifiers` `/properties/TopicName`. Either is resolved to the one topic it
/// names, and whichever of them addressed it, every answer reports the topic's own ARN.
///
/// A value that is neither — an ARN of another account, Region or service, a name no topic
/// could carry — names no topic, which is what a read of it reports as not found.
fn topic_arn_of(region: &str, identifier: &str) -> Option<String> {
    match sns::topic_name_from_arn(identifier) {
        // An ARN: held to the one THIS Region and account assign for that name, so an ARN
        // of another namespace names no topic this endpoint answers for.
        Some(name) => (identifier == sns::topic_arn(region, name)).then(|| identifier.to_string()),
        // Otherwise the topic's name, the type's other declared identifier.
        None => sns::topic_name_is_valid(identifier).then(|| sns::topic_arn(region, identifier)),
    }
}

/// How a refusal states what the type's primary identifier has to be, for a create whose
/// identifier names no topic.
fn topic_arn_requirement() -> String {
    format!(
        "must be the ARN of a topic of this account in the form {}, or the topic's name",
        sns::topic_arn("<region>", "<TopicName>")
    )
}

/// The name a create picks for a topic whose model names none — the shared rule, carrying
/// the suffix a FIFO topic's name must end with.
///
/// Whether a model's `FifoTopic` value designates a FIFO topic: the schema declares the
/// property a boolean and a topic with no such property is a standard one, so a model
/// carrying `true` asks for a FIFO topic and anything else asks for a standard one.
fn designates_fifo(value: Option<&Value>) -> bool {
    value.is_some_and(|value| value.as_bool() == Some(true))
}

/// The resource model one topic renders as — the schema's properties the owning record
/// carries, and only those it HAS: a topic under no display name, no key and no tags
/// carries none of those properties, the same way the owning service's own reader reports
/// no attribute for them.
fn model(topic: &sns::TopicRecord) -> Value {
    let mut model = Map::new();
    model.insert(
        IDENTIFIER_PROPERTY.to_string(),
        Value::from(topic.arn.clone()),
    );
    model.insert(TOPIC_NAME.to_string(), Value::from(topic.name.clone()));
    // Ordered, so one topic renders the same model whichever order the owning store's hash
    // map happens to hand its attributes over in.
    let carried: BTreeMap<&String, &String> = topic.attributes.iter().collect();
    for (attribute, value) in carried {
        if let Some(form) = applied_property(attribute) {
            model.insert(attribute.clone(), form.property_value(value));
        }
    }
    // The topic's kind is what the owning record holds, whether or not its create carried
    // the attribute for it.
    if topic.is_fifo {
        model.insert(FIFO_TOPIC.to_string(), Value::Bool(true));
    }
    if !topic.tags.is_empty() {
        model.insert(TAGS.to_string(), tag_list(topic.tags.iter()));
    }
    Value::Object(model)
}

/// The tag set a write would leave the topic holding, held to the owning service's own
/// rules: its documented per-topic tag quota, its record ceiling, and the key namespace it
/// keeps for Amazon Web Services' own tags.
fn tags_accepted(tags: &HashMap<String, String>) -> Result<(), NativeRefusal> {
    if let Some(refusal) = sns::tag_map_refusal(tags) {
        return Err(NativeRefusal::Invalid(match refusal {
            sns::TagRefusal::TooMany(message) | sns::TagRefusal::TooLarge(message) => message,
        }));
    }
    // A resource model's tags are the ones a CALLER named, so the reserved namespace is
    // refused here for the same reason the owning service's own tagging operations refuse
    // it: nothing writes a tag set into its store that its own API would not take.
    if let Some(message) = sns::reserved_tag_key_refusal(
        TAGS,
        tags.keys().map(String::as_str),
        sns::TagKeyOrigin::Caller,
    ) {
        return Err(NativeRefusal::Invalid(message));
    }
    Ok(())
}

/// The attribute map a write would leave the topic holding, held to the owning service's
/// own per-record ceiling.
fn record_fits(attributes: &HashMap<String, String>) -> Result<(), NativeRefusal> {
    match sns::attribute_bytes_refusal("a topic", attributes) {
        Some(message) => Err(NativeRefusal::Invalid(message)),
        None => Ok(()),
    }
}

/// What a resource model asks a topic to be: the attribute map the owning service's own
/// calls take, and the tag set the topic is to carry, each read off the model's properties.
struct DesiredModel {
    /// The topic's attributes, as the owning service holds them: one entry per attribute
    /// property the model carried, holding that value's text.
    supplied: HashMap<String, String>,
    /// The tag set the topic is to carry.
    tags: BTreeMap<String, String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the topic called `name`, whose ARN is `arn` — or
    /// state why the owning service would not take it.
    ///
    /// The identity and the read-only ARN may be CARRIED (a read reports them, so a model a
    /// client round-trips carries them) but only with the value the topic already answers
    /// to: a resource never answers to an identifier other than the one it is stored under,
    /// and nothing here renames a topic.
    fn read(name: &str, arn: &str, properties: &Value) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            supplied: HashMap::new(),
            tags: BTreeMap::new(),
        };
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, arn)?,
                TOPIC_NAME => require_value(property, value, name)?,
                TAGS => desired.tags = tag_set(value)?,
                // Properties of the type the OWNING service keeps outside the topic's
                // attribute map, each written by a call of its own; see this module's
                // header.
                SUBSCRIPTION | DATA_PROTECTION_POLICY | DELIVERY_STATUS_LOGGING => {
                    return Err(unsupported(TYPE_NAME, property));
                }
                other => match applied_property(other) {
                    Some(form) => {
                        desired
                            .supplied
                            .insert(other.to_string(), form.attribute_text(other, value)?);
                    }
                    None => {
                        return Err(NativeRefusal::Invalid(format!(
                            "{other} is not a property of {TYPE_NAME}"
                        )));
                    }
                },
            }
        }
        Ok(desired)
    }

    /// Whether this model asks for a FIFO topic, which is what makes the FIFO-only
    /// attributes applicable to it.
    fn designates_fifo(&self) -> bool {
        self.supplied
            .get(FIFO_TOPIC)
            .is_some_and(|value| sns::flag_attribute_enabled(value))
    }

    /// The tag set this model asks for, as the owning service holds one.
    fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The attribute map this model would leave the topic carrying, every entry of it held
    /// to the OWNING service's own rule for that attribute on a topic of this kind holding
    /// these subscriptions — or why that service would not take it.
    ///
    /// A resource model is the WHOLE desired state of the resource, so an attribute the
    /// model does not carry is one the topic does not hold afterwards: that is what makes a
    /// model a client round-trips describe the topic it reads back. A FIFO topic keeps the
    /// attribute that says so, because its kind is the owning record's and not a value this
    /// map can clear.
    fn attributes(
        &self,
        fifo: bool,
        subscription_protocols: &[String],
    ) -> Result<HashMap<String, String>, NativeRefusal> {
        let subject = sns::topic_attributes::TopicSubject {
            is_fifo: fifo,
            subscription_protocols,
        };
        // In attribute-name order, so a model breaking two rules is refused by the same one
        // of them whichever order the model's own properties arrived in.
        let judged: BTreeMap<&String, &String> = self.supplied.iter().collect();
        for (name, value) in judged {
            if let Some(refusal) = sns::topic_attributes::topic_attribute_refusal(name, value, subject)
            {
                return Err(NativeRefusal::Invalid(refusal.message().to_string()));
            }
        }
        let mut attributes = self.supplied.clone();
        if fifo {
            attributes.insert(FIFO_TOPIC.to_string(), true.to_string());
        }
        Ok(attributes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property this path applies is an attribute the OWNING service defines for a
    /// topic, so nothing here writes a name that service's own API would refuse.
    #[test]
    fn every_applied_property_is_an_attribute_the_owning_service_defines() {
        for (property, _) in APPLIED_PROPERTIES {
            assert!(
                sns::is_topic_attribute(property),
                "property {property:?}: not an attribute Amazon SNS defines for a topic"
            );
        }
    }

    /// A reference resolves to the ARN the owning service assigns, and a value that names
    /// no topic of this account and Region resolves to none.
    #[test]
    fn a_reference_resolves_to_the_arn_the_owning_service_assigned() {
        let region = "us-east-1";
        let arn = sns::topic_arn(region, "demo");
        // The rows differ along the form of the reference: the assigned ARN, the bare name,
        // a FIFO topic's name, an ARN of another Region, an ARN of another account, an ARN
        // of another service, and a name no topic may carry.
        let rows = [
            ("the assigned ARN", arn.clone(), Some(arn.clone())),
            ("the topic's name", "demo".to_string(), Some(arn.clone())),
            (
                "a FIFO topic's name",
                "demo.fifo".to_string(),
                Some(sns::topic_arn(region, "demo.fifo")),
            ),
            (
                "an ARN of another Region",
                sns::topic_arn("eu-west-1", "demo"),
                None,
            ),
            (
                "an ARN of another account",
                "arn:aws:sns:us-east-1:999999999999:demo".to_string(),
                None,
            ),
            (
                "an ARN of another service",
                "arn:aws:sqs:us-east-1:000000000000:demo".to_string(),
                None,
            ),
            ("a name no topic may carry", "not a name".to_string(), None),
        ];
        for (case, reference, expected) in rows {
            assert_eq!(
                topic_arn_of(region, &reference),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                topic_arn_of(region, &reference)
            );
        }
    }
}
