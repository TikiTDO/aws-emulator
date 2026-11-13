// SPDX-License-Identifier: CC0-1.0

//! The `{{resolve:...}}` DYNAMIC REFERENCES a template's resource properties carry, and
//! the store each one is resolved against.
//!
//! ## The rule this module implements
//!
//! A template may stand a property's value in as a dynamic reference rather than write the
//! value itself: `{{resolve:<store>:<reference key>}}`. CloudFormation resolves it ITSELF,
//! against the store the reference names, and the FORM of the reference is what decides
//! which values that store may answer with — the `ssm` form resolves a Parameter Store
//! parameter held as plain text (a `String` or a `StringList`), the `ssm-secure` form
//! resolves only one held encrypted (a `SecureString`), and the `secretsmanager` form
//! resolves a secret AWS Secrets Manager holds, or one key of the JSON document it holds —
//! as well as WHERE the reference may stand: a Parameter Store SECURE one is supported only
//! in the resource properties AWS publishes for it ([`SSM_SECURE_PROPERTIES`]).
//!
//! ## When each form is resolved
//!
//! A reference whose store stands OUTSIDE the deployment is resolved before the deployment
//! asks any owning service for anything ([`resolve_declared`]): nothing it names can be
//! created by the very template carrying it, so one it cannot honour — the named store holds
//! no such parameter, or holds one of a type that form does not resolve, or a secure
//! reference stands in an unsupported property — is a MISUSE of the reference rather than a
//! value, and the misuse is CLOUDFORMATION's own refusal: no resource provider is asked
//! about any of the deployment's resources, and the diagnostic saying WHICH reference could
//! not be honoured and why is what the stack itself reports as its status reason, so a
//! create rolls back carrying the resolver's words rather than a summary of resources that
//! tried and failed ([`super::provisioning::reconcile`] is what records that).
//!
//! A `secretsmanager` reference is resolved LATER: as the resource whose property carries it
//! is created or updated ([`resolve_as_deployed`]), because the secret it names may be one
//! the same deployment creates — a secret declared beside the resource that is configured
//! with it, which exists only once that resource's dependency has been created. Such a
//! reference is therefore resolved against the secret in force at the moment the owning
//! service is asked for the resource, and one that cannot be honoured fails THAT resource,
//! carrying the diagnostic, rather than the deployment as a whole.
//!
//! The store is read through the SAME in-process, signed call a resource's own create rides
//! on ([`super::provisioner::call_ssm`],
//! [`super::provisioning::call_secretsmanager`]), so a reference resolves against the very
//! parameters `PutParameter` wrote and the very secret `CreateSecret` minted rather than
//! against a second copy of them.
//!
//! ## What the stack records
//!
//! The whole point of a secret form is that the secret stays in the store it is held in,
//! out of the template and out of the stack. [`Resolution`] is that split: the document the
//! service that OWNS the resource is asked to create it with carries the resolved value,
//! and the document the stack RECORDS and reports keeps the reference exactly as the
//! template wrote it. Only a plain `ssm` value — which the store itself serves as plain
//! text to any caller — appears in both.
//!
//! ## What this module leaves alone
//!
//! A reference naming a store this module does not resolve passes through verbatim, as does
//! a text that is not a reference at all: nothing here guesses at a value, and nothing here
//! fails a resource over a store it never asked.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::provisioner::call_ssm;
use super::provisioning::call_secretsmanager;

/// The delimiters of a dynamic reference: `{{resolve:<store>:<reference key>}}`.
const OPEN: &str = "{{resolve:";

/// See [`OPEN`].
const CLOSE: &str = "}}";

/// The `ssm` store: "Systems Manager parameters of type `String` and `StringList`".
const SSM: &str = "ssm";

/// The `ssm-secure` store: Systems Manager parameters of type `SecureString`.
const SSM_SECURE: &str = "ssm-secure";

/// The `secretsmanager` store: a secret AWS Secrets Manager holds, whose reference is
/// written `{{resolve:secretsmanager:<secret id>:<secret string>:<json key>:<version
/// stage>:<version id>}}` — the secret's name or ARN, then the four optional fields
/// [`Named`] reads.
const SECRETSMANAGER: &str = "secretsmanager";

/// The Systems Manager operation one reference is resolved with.
const GET_PARAMETER: &str = "GetParameter";

/// The Secrets Manager operation one reference of that store is resolved with: the read
/// whose `SecretId`, `VersionStage` and `VersionId` members are the fields the reference
/// carries (`smithy_query(query="operation",operation="GetSecretValue")` against
/// `services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/`).
const GET_SECRET_VALUE: &str = "GetSecretValue";

/// The only `secret-string` field a `secretsmanager` reference may name: "Currently, the
/// only supported value is `SecretString`", which is also the member of that read the value
/// is taken from.
const SECRET_STRING: &str = "SecretString";

/// The head every ARN is written with, which is what says a reference's secret id is an ARN
/// rather than a secret NAME ([`Named::split_secret_id`]).
const ARN: &str = "arn:";

/// How many colon-separated segments stand between an ARN's head and its end:
/// `arn:<partition>:<service>:<region>:<account>:secret:<name>-<suffix>` carries six of them
/// after `arn:`, and a secret's name cannot carry a colon of its own, so the sixth colon
/// after the head is where the secret id ends and the reference's own fields begin.
const ARN_SEGMENTS_AFTER_THE_HEAD: usize = 6;

/// The `ParameterType` values a Parameter Store parameter is held under, from that
/// service's own modelled enum (`smithy_query(query="shape",shape="ParameterType")`
/// against `services/service-com-amazonaws-ssm-amazonssm/smithy-model/`): the three types
/// a parameter can have, which is what decides whether a reference's form resolves it.
const PARAMETER_TYPE_STRING: &str = "String";

/// See [`PARAMETER_TYPE_STRING`].
const PARAMETER_TYPE_STRING_LIST: &str = "StringList";

/// See [`PARAMETER_TYPE_STRING`].
const PARAMETER_TYPE_SECURE_STRING: &str = "SecureString";

/// How many dynamic references one resource's properties may carry.
///
/// Emulator resource guard, NOT a published AWS quota: each distinct reference costs one
/// in-process read of the store and substitutes a value far longer than the reference's own
/// text, so without a bound a caller-sized template could ask for an unbounded number of
/// reads and grow one resource's document without limit. Set well above what a real
/// declaration carries, so no template a client would write reaches it.
const MAX_REFERENCES: usize = 256;

/// The head of every SSM PARAMETER TYPE a template's `Parameters` section can declare:
/// `AWS::SSM::Parameter::Value<String>`, `…<List<String>>`, `…<AWS::EC2::Image::Id>` and the
/// rest of the published list all begin with it.
///
/// "AWS-specific parameter types … AWS::SSM::Parameter::Value<String>: The parameter value
/// corresponds to an existing Systems Manager parameter in Parameter Store" — so what the
/// request (or the declaration's `Default`) supplies for such a parameter is the NAME of a
/// Parameter Store parameter, and the value the deployment runs with is that parameter's own
/// value.
///
/// Resolving one is [`super::ssm_parameters`], which is the ONE reader of that rule: it
/// records the value the store answered on the parameter itself, so the stack reports it as
/// `Parameter$ResolvedValue` and every later evaluation of the stack's own outputs and
/// resources resolves to it. Only the SPELLING of the type head lives here, beside the
/// `{{resolve:ssm:…}}` forms it sits next to.
pub const SSM_PARAMETER_TYPE: &str = "AWS::SSM::Parameter::Value<";

/// One resolved reference: the value the store answered, and whether that value is one the
/// stack must keep out of its own records (see the module docs).
#[derive(Debug, Clone)]
struct Resolved {
    value: String,
    secret: bool,
}

/// One resource's properties with its dynamic references resolved, in the two documents a
/// deployment needs them in.
#[derive(Debug, Clone)]
pub(super) struct Resolution {
    /// What the service that OWNS the resource is asked to create or update it with: every
    /// reference this module resolved replaced by the value its store answered.
    pub(super) deployed: Value,
    /// What the stack RECORDS for the resource, and so what its reads and its events
    /// report: as `deployed`, except that a reference whose value is a secret is left
    /// exactly as the template wrote it.
    pub(super) recorded: Value,
}

impl Resolution {
    /// The resolution of a document carrying no dynamic reference this module resolves: it
    /// is deployed and recorded exactly as the declaration wrote it.
    pub(super) fn verbatim(properties: &Value) -> Self {
        Self {
            deployed: properties.clone(),
            recorded: properties.clone(),
        }
    }
}

/// Resolve the dynamic references of EVERY resource a deployment declares, in the order the
/// template declares them, and before anything is asked of any owning service.
///
/// This is CloudFormation's OWN gate on a deployment (see the module docs): `Err(reason)` is
/// a reference the deployment cannot honour, worded as the diagnostic the STACK reports as
/// its own status reason — no resource of the deployment is deployed at all, so the failure
/// is not attributed to a resource provider that was never asked.
///
/// `Ok` is one [`Resolution`] per declared resource, keyed by the logical id the template
/// declares it under: the documents that deployment then deploys and records the resource
/// with.
pub(super) async fn resolve_declared(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    declared: &[super::StoredResource],
) -> Result<BTreeMap<String, Resolution>, String> {
    let mut resolutions = BTreeMap::new();
    for resource in declared {
        resolutions.insert(
            resource.logical_id.clone(),
            resolve(
                ctx,
                region,
                &resource.resource_type,
                &resource.properties,
                Pass::BeforeTheDeployment,
            )
            .await?,
        );
    }
    Ok(resolutions)
}

/// Resolve the dynamic references ONE resource's properties carry that are resolved as that
/// resource is deployed ([`Pass::AsTheResourceIsDeployed`]), in the document the service
/// that OWNS it is asked for it with.
///
/// `properties` is that document, already carrying the identifiers the resources this
/// deployment created before this one really have, which is why this pass runs here: a
/// `secretsmanager` reference can name a secret the SAME deployment created, and the secret
/// exists — and is addressed by the ARN Secrets Manager minted for it — only once that
/// resource has been created.
///
/// The resolved value is a secret, so only this document carries it: the record the stack
/// keeps of the resource is the caller's and keeps the reference exactly as the template
/// wrote it.
///
/// `Err(reason)` is a reference this resource cannot be deployed with, worded as the
/// `ResourceStatusReason` that resource then fails with — CloudFormation has no value to ask
/// its owning service for.
pub(super) async fn resolve_as_deployed(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_type: &str,
    properties: &mut Value,
) -> Result<(), String> {
    let resolution = resolve(
        ctx,
        region,
        resource_type,
        properties,
        Pass::AsTheResourceIsDeployed,
    )
    .await?;
    *properties = resolution.deployed;
    Ok(())
}

/// Resolve every dynamic reference `properties` carries that `pass` is the pass for, against
/// the store it names.
///
/// `resource_type` is the type of the resource whose properties these are, which is what
/// decides WHERE a secure reference may stand (see [`SSM_SECURE_PROPERTIES`]).
///
/// `Err(reason)` is a reference this resource cannot be deployed with — one the named store
/// cannot satisfy, or a secure one standing somewhere it is not supported — worded as the
/// `ResourceStatusReason` the failure is reported with: such a reference has no value, so
/// the deployment fails rather than asking an owning service for a resource configured with
/// the reference's own text.
async fn resolve(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_type: &str,
    properties: &Value,
    pass: Pass,
) -> Result<Resolution, String> {
    let mut occurrences = Vec::new();
    collect(properties, "", 0, pass, &mut occurrences);
    if occurrences.is_empty() {
        // The common case: nothing to resolve, so both documents are the declaration's.
        return Ok(Resolution::verbatim(properties));
    }
    if occurrences.len() > MAX_REFERENCES {
        return Err(format!(
            "Unable to resolve the dynamic references of this resource: its properties carry \
             more than the {MAX_REFERENCES} this emulator resolves for one resource"
        ));
    }
    // WHERE each reference stands is judged before anything is read: a secure reference
    // outside the properties that support it is refused whatever the store holds.
    for (path, reference) in &occurrences {
        let Some((form, _)) = parsed(reference) else {
            continue;
        };
        if form.restricted_to_published_properties()
            && !secure_reference_supported(resource_type, path)
        {
            return Err(format!(
                "SSM Secure reference is not supported in: \
                 [{resource_type}/Properties/{path}]"
            ));
        }
    }
    // Each DISTINCT reference is read once, however many properties name it.
    let references: BTreeSet<&String> =
        occurrences.iter().map(|(_, reference)| reference).collect();
    let mut resolutions: BTreeMap<String, Resolved> = BTreeMap::new();
    for reference in references {
        let Some((form, key)) = parsed(reference) else {
            continue;
        };
        resolutions.insert(
            reference.clone(),
            held_value(ctx, region, reference, form, key).await?,
        );
    }
    Ok(Resolution {
        deployed: substituted(properties, &resolutions, true, 0),
        recorded: substituted(properties, &resolutions, false, 0),
    })
}

/// The resource properties a SECURE reference may stand in, as AWS publishes them — "you
/// can use the `ssm-secure` dynamic reference in the following resource properties" in
/// "Specify values stored in other services using dynamic references".
///
/// A secure reference is not a value a resource provider may be handed anywhere: the
/// secret is only kept out of the stack where the provider treats the property as one
/// (every property here is a password, a token or a key no read reports back). Anywhere
/// else CloudFormation refuses the reference rather than resolving a secret into a
/// property that would be reported.
///
/// Each entry is the resource type and the PATH through its `Properties` the reference may
/// stand at, with `/` between the members ([`property_path`] builds the same spelling for
/// the reference being judged).
const SSM_SECURE_PROPERTIES: &[(&str, &[&str])] = &[
    ("AWS::DirectoryService::MicrosoftAD", &["Password"]),
    ("AWS::DirectoryService::SimpleAD", &["Password"]),
    ("AWS::ElastiCache::ReplicationGroup", &["AuthToken"]),
    ("AWS::IAM::User", &["LoginProfile/Password"]),
    (
        "AWS::KinesisFirehose::DeliveryStream",
        &[
            "RedshiftDestinationConfiguration/Password",
            "SplunkDestinationConfiguration/HECToken",
        ],
    ),
    ("AWS::OpsWorks::App", &["Source/Password"]),
    (
        "AWS::OpsWorks::Stack",
        &[
            "CustomCookbooksSource/Password",
            "RdsDbInstances/DbPassword",
        ],
    ),
    ("AWS::RDS::DBCluster", &["MasterUserPassword"]),
    ("AWS::RDS::DBInstance", &["MasterUserPassword"]),
    ("AWS::Redshift::Cluster", &["MasterUserPassword"]),
];

/// Whether a secure reference standing at `path` in a `resource_type`'s properties is one
/// of the supported ones ([`SSM_SECURE_PROPERTIES`]).
///
/// A path through a LIST carries that element's index, which is not part of the property it
/// names (one `RdsDbInstances` entry's `DbPassword` is the same property as another's), so
/// the index segments are dropped before the comparison.
fn secure_reference_supported(resource_type: &str, path: &str) -> bool {
    let named: Vec<&str> = path
        .split('/')
        .filter(|segment| segment.parse::<usize>().is_err())
        .collect();
    SSM_SECURE_PROPERTIES
        .iter()
        .filter(|(supported_type, _)| *supported_type == resource_type)
        .flat_map(|(_, properties)| properties.iter())
        .any(|supported| supported.split('/').eq(named.iter().copied()))
}

/// The path of one member or list element under `parent`, as
/// [`SSM_SECURE_PROPERTIES`] spells a property path.
fn property_path(parent: &str, segment: &str) -> String {
    if parent.is_empty() {
        segment.to_string()
    } else {
        format!("{parent}/{segment}")
    }
}

/// Read the value one reference names, from the store its FORM names.
async fn held_value(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    reference: &str,
    form: Form,
    key: &str,
) -> Result<Resolved, String> {
    match form {
        Form::Ssm | Form::SsmSecure => parameter(ctx, region, reference, form, key).await,
        Form::SecretsManager => secret(ctx, region, reference, key).await,
    }
}

/// Read the parameter one reference names, and hold the answer to the types that
/// reference's FORM resolves.
///
/// The read is `GetParameter` on the store's own wire, so a parameter it does not hold
/// answers with its own `ParameterNotFound` and a version it does not hold with its own
/// `ParameterVersionNotFound` — the refusal the failure then carries is that service's,
/// exactly as a refused resource create carries its owning service's.
async fn parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    reference: &str,
    form: Form,
    key: &str,
) -> Result<Resolved, String> {
    if key.is_empty() {
        return Err(format!(
            "Unable to resolve the dynamic reference '{reference}': it names no parameter"
        ));
    }
    // The reference key is the name the store is asked for, verbatim: Parameter Store
    // itself reads a `name:version` suffix as the version selector and a full ARN as the
    // parameter it names, which are the two spellings a reference key takes, so neither is
    // re-parsed here.
    let request = json!({ "Name": key, "WithDecryption": true });
    let answer = call_ssm(ctx, region, GET_PARAMETER, &request)
        .await
        .map_err(|refusal| {
            format!("Unable to resolve the dynamic reference '{reference}': {refusal}")
        })?;
    let held = answer.pointer("/Parameter/Type").and_then(Value::as_str);
    let value = answer.pointer("/Parameter/Value").and_then(Value::as_str);
    let (Some(held), Some(value)) = (held, value) else {
        return Err(format!(
            "Unable to resolve the dynamic reference '{reference}': the parameter store \
             reported no type and value for {key}"
        ));
    };
    if !form.resolves().contains(&held) {
        return Err(form.misuse(key));
    }
    Ok(Resolved {
        value: value.to_string(),
        secret: form.secret(),
    })
}

/// Read the secret one `secretsmanager` reference names, and take the value the reference
/// asks for out of it.
///
/// The read is `GetSecretValue` on Secrets Manager's own wire, carrying the version the
/// reference selects, so a secret that service does not hold answers with its own
/// `ResourceNotFoundException` and a version it does not hold with its own refusal — the
/// reason the failure then carries is that service's, exactly as a refused resource create
/// carries its owning service's.
///
/// A reference naming a `json-key` resolves to THAT key of the JSON document the secret
/// holds — "the key name of the key-value pair whose value you want to retrieve" — and one
/// naming none resolves to the whole secret text. A secret holding no text at all (one held
/// as `SecretBinary`), one whose text is not a JSON document, and one whose document has no
/// such key each leave the reference with no value, so each fails the resource rather than
/// configuring it with the reference's own text.
async fn secret(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    reference: &str,
    key: &str,
) -> Result<Resolved, String> {
    let unresolvable =
        |detail: String| format!("Unable to resolve the dynamic reference '{reference}': {detail}");
    let named = Named::of(key).map_err(unresolvable)?;
    let mut request = Map::new();
    request.insert(
        "SecretId".to_string(),
        Value::String(named.secret_id.to_string()),
    );
    if let Some(stage) = named.version_stage {
        request.insert("VersionStage".to_string(), Value::String(stage.to_string()));
    }
    if let Some(version) = named.version_id {
        request.insert("VersionId".to_string(), Value::String(version.to_string()));
    }
    let answer = call_secretsmanager(ctx, region, GET_SECRET_VALUE, &Value::Object(request))
        .await
        .map_err(|refusal| unresolvable(refusal.to_string()))?;
    let Some(held) = answer.get(SECRET_STRING).and_then(Value::as_str) else {
        return Err(unresolvable(format!(
            "the secret {} holds no {SECRET_STRING}",
            named.secret_id
        )));
    };
    let Some(json_key) = named.json_key else {
        return Ok(Resolved {
            value: held.to_string(),
            secret: true,
        });
    };
    let document: Value = serde_json::from_str(held).map_err(|_| {
        unresolvable(format!(
            "the secret {} does not hold a JSON document, so it has no {json_key} key",
            named.secret_id
        ))
    })?;
    let Some(member) = document.get(json_key) else {
        return Err(unresolvable(format!(
            "the secret {} holds no {json_key} key",
            named.secret_id
        )));
    };
    Ok(Resolved {
        // A key whose value the document writes as a string resolves to that string; one
        // written as a number or a boolean resolves to how the document writes it, since a
        // property carries text either way.
        value: match member {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        },
        secret: true,
    })
}

/// What one `secretsmanager` reference names, read out of its reference key:
/// `<secret id>:<secret string>:<json key>:<version stage>:<version id>`, of which only the
/// secret id is required and an omitted field is written as nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Named<'a> {
    /// "The name or ARN of the secret", which is what the read is addressed by.
    secret_id: &'a str,
    /// The key of the JSON document the secret holds to resolve to, or `None` for a
    /// reference that resolves to the whole secret text.
    json_key: Option<&'a str>,
    /// The version STAGE to read, or `None` for the one Secrets Manager serves by default.
    version_stage: Option<&'a str>,
    /// The version ID to read, which a reference states INSTEAD of a stage.
    version_id: Option<&'a str>,
}

impl<'a> Named<'a> {
    /// What `key` names, or the diagnostic a reference that names no value is failed with.
    fn of(key: &'a str) -> Result<Self, String> {
        let (secret_id, fields) = Self::split_secret_id(key);
        if secret_id.is_empty() {
            return Err("it names no secret".to_string());
        }
        let mut fields = fields.split(':');
        let stated = |field: Option<&'a str>| field.filter(|field| !field.is_empty());
        let secret_string = stated(fields.next());
        let named = Self {
            secret_id,
            json_key: stated(fields.next()),
            version_stage: stated(fields.next()),
            version_id: stated(fields.next()),
        };
        if let Some(unsupported) = secret_string.filter(|field| *field != SECRET_STRING) {
            return Err(format!(
                "the only supported value for its secret-string field is {SECRET_STRING}, not \
                 {unsupported}"
            ));
        }
        if fields.next().is_some() {
            return Err(format!(
                "it carries more fields than \
                 <secret id>:{SECRET_STRING}:<json key>:<version stage>:<version id>"
            ));
        }
        // "You can specify either version-stage or version-id, but not both."
        if named.version_stage.is_some() && named.version_id.is_some() {
            return Err(
                "it states both a version-stage and a version-id, and a reference may state \
                 either but not both"
                    .to_string(),
            );
        }
        Ok(named)
    }

    /// The secret id a reference key opens with, and the fields that follow it.
    ///
    /// A secret is named by its NAME or by its ARN, and an ARN carries colons of its own:
    /// the fields of such a key begin after the ARN's last segment
    /// ([`ARN_SEGMENTS_AFTER_THE_HEAD`]) rather than at the first colon, so a reference
    /// written with a full ARN names the same secret a reference written with its name does.
    fn split_secret_id(key: &'a str) -> (&'a str, &'a str) {
        let Some(after_the_head) = key.strip_prefix(ARN) else {
            return match key.split_once(':') {
                Some((secret_id, fields)) => (secret_id, fields),
                None => (key, ""),
            };
        };
        match after_the_head
            .match_indices(':')
            .nth(ARN_SEGMENTS_AFTER_THE_HEAD - 1)
        {
            Some((offset, _)) => (
                &key[..ARN.len() + offset],
                &after_the_head[offset + ':'.len_utf8()..],
            ),
            // The whole key is the ARN: the reference names no field at all.
            None => (key, ""),
        }
    }
}

/// The store a dynamic reference names, of the ones this module resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `{{resolve:ssm:<parameter name>}}` or `{{resolve:ssm:<parameter name>:<version>}}`.
    Ssm,
    /// `{{resolve:ssm-secure:<parameter name>}}` or
    /// `{{resolve:ssm-secure:<parameter name>:<version>}}`.
    SsmSecure,
    /// `{{resolve:secretsmanager:<secret id>:<secret string>:<json key>:<version
    /// stage>:<version id>}}` ([`Named`]).
    SecretsManager,
}

impl Form {
    /// The form `store` names, or `None` for a store this module does not resolve.
    fn of(store: &str) -> Option<Self> {
        match store {
            SSM => Some(Self::Ssm),
            SSM_SECURE => Some(Self::SsmSecure),
            SECRETSMANAGER => Some(Self::SecretsManager),
            _ => None,
        }
    }

    /// The parameter TYPES this form resolves — the whole rule the Parameter Store forms are
    /// about, in one place: the plain form resolves the two plain-text types and the secure
    /// form resolves only the encrypted one, so neither reads a value the other's guarantee
    /// covers.
    fn resolves(self) -> &'static [&'static str] {
        match self {
            Self::Ssm => &[PARAMETER_TYPE_STRING, PARAMETER_TYPE_STRING_LIST],
            Self::SsmSecure => &[PARAMETER_TYPE_SECURE_STRING],
            // Not a Parameter Store form: what it resolves is a secret, and no parameter
            // type decides anything about it.
            Self::SecretsManager => &[],
        }
    }

    /// Whether a value this form resolves is a secret, which the stack keeps out of its own
    /// records (see the module docs).
    fn secret(self) -> bool {
        matches!(self, Self::SsmSecure | Self::SecretsManager)
    }

    /// Whether this form may stand ONLY in the resource properties AWS publishes for it
    /// ([`SSM_SECURE_PROPERTIES`]) — the Parameter Store secure form, which AWS publishes
    /// such a list for. The `secretsmanager` form carries no such list: it stands in any
    /// resource property.
    fn restricted_to_published_properties(self) -> bool {
        matches!(self, Self::SsmSecure)
    }

    /// Whether this form is resolved as the resource carrying it is DEPLOYED rather than by
    /// the gate that runs before the deployment asks any owning service for anything (see
    /// the module docs): the `secretsmanager` store can hold a secret the same deployment
    /// creates, and the two Parameter Store forms cannot.
    fn resolved_as_the_resource_is_deployed(self) -> bool {
        matches!(self, Self::SecretsManager)
    }

    /// The `ResourceStatusReason` a reference of this form pointed at a parameter of a type
    /// it does not resolve fails with: the PREFIX the reference used, against the kind of
    /// parameter the store holds under that name, which is the name the request itself
    /// carried.
    fn misuse(self, key: &str) -> String {
        match self {
            Self::Ssm => format!("ssm prefix was used for secure parameter {key}"),
            Self::SsmSecure => {
                format!("ssm-secure prefix was used for non-secure parameter {key}")
            }
            Self::SecretsManager => {
                format!("secretsmanager prefix was used for parameter {key}")
            }
        }
    }
}

/// WHICH of a document's dynamic references one resolution pass is about — the two moments
/// this module resolves a reference at (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Before the deployment asks any owning service for anything: every form whose store
    /// stands outside the deployment.
    BeforeTheDeployment,
    /// As the resource whose property carries the reference is created or updated: the forms
    /// whose store can hold a value the same deployment creates.
    AsTheResourceIsDeployed,
}

impl Pass {
    /// Whether a reference of `form` is one this pass resolves.
    fn covers(self, form: Form) -> bool {
        form.resolved_as_the_resource_is_deployed() == matches!(self, Self::AsTheResourceIsDeployed)
    }
}

/// The store and the reference key `reference` names, for a reference this module resolves;
/// `None` for anything else — another store's reference, or a text that only looks like
/// one.
fn parsed(reference: &str) -> Option<(Form, &str)> {
    let inner = reference.strip_prefix(OPEN)?.strip_suffix(CLOSE)?;
    let (store, key) = inner.split_once(':')?;
    Some((Form::of(store)?, key))
}

/// Every occurrence of a dynamic reference `pass` resolves that `value` carries: the
/// PATH through the document it stands at, and the reference's own text.
fn collect(value: &Value, path: &str, depth: usize, pass: Pass, into: &mut Vec<(String, String)>) {
    if depth > super::template::MAX_TEMPLATE_DEPTH || into.len() > MAX_REFERENCES {
        return;
    }
    match value {
        // A reference stands in for a VALUE, so only a string carries one: a property NAME
        // is not a value, and a template cannot write one as a number or a boolean.
        Value::String(text) => references_in(text, path, pass, into),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect(
                    item,
                    &property_path(path, &index.to_string()),
                    depth + 1,
                    pass,
                    into,
                );
            }
        }
        Value::Object(members) => {
            for (name, member) in members {
                collect(member, &property_path(path, name), depth + 1, pass, into);
            }
        }
        _ => {}
    }
}

/// Every dynamic reference `pass` resolves that one text carries, in the SAME
/// left-to-right scan [`substituted_text`] replaces them in — so a reference that is
/// collected is one that gets substituted, and the reverse.
///
/// The collection bound holds INSIDE one text as well as across the document: a single
/// property value can carry as many references as its own length allows, each costing an
/// entry that clones the path it stands at, so the scan stops at the bound rather than
/// growing the set to the size of the text it is reading. The set the scan hands back has
/// then PASSED the bound, which is what [`resolve`] refuses the whole document on.
fn references_in(text: &str, path: &str, pass: Pass, into: &mut Vec<(String, String)>) {
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        if into.len() > MAX_REFERENCES {
            return;
        }
        let at = &rest[start..];
        let Some(end) = at.find(CLOSE) else {
            break;
        };
        let (reference, after) = at.split_at(end + CLOSE.len());
        if parsed(reference).is_some_and(|(form, _)| pass.covers(form)) {
            into.push((path.to_string(), reference.to_string()));
        }
        rest = after;
    }
}

/// `value` with every resolved reference replaced by the value its store answered.
///
/// `secrets` is whether a value the stack keeps out of its records is substituted too:
/// true for the document the owning service is asked for the resource with, false for the
/// one the stack records.
fn substituted(
    value: &Value,
    resolutions: &BTreeMap<String, Resolved>,
    secrets: bool,
    depth: usize,
) -> Value {
    if depth > super::template::MAX_TEMPLATE_DEPTH {
        return value.clone();
    }
    match value {
        Value::String(text) => Value::String(substituted_text(text, resolutions, secrets)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substituted(item, resolutions, secrets, depth + 1))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| {
                    (
                        name.clone(),
                        substituted(member, resolutions, secrets, depth + 1),
                    )
                })
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

/// One text with every resolved reference replaced in place, so a reference inside a longer
/// value (a connection string around a password) substitutes the value and keeps the text
/// around it.
///
/// ONE left-to-right pass: a value a store answered is appended and never scanned again, so
/// a stored value that happens to carry the text of another reference is a value and not a
/// reference. A reference with no resolution — another store's, or one past the collection
/// bound — is written back exactly as the template had it.
fn substituted_text(
    text: &str,
    resolutions: &BTreeMap<String, Resolved>,
    secrets: bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = rest.find(OPEN) else {
            out.push_str(rest);
            break;
        };
        let (before, at) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = at.find(CLOSE) else {
            out.push_str(at);
            break;
        };
        let (reference, after) = at.split_at(end + CLOSE.len());
        match resolutions.get(reference) {
            Some(resolved) if secrets || !resolved.secret => out.push_str(&resolved.value),
            _ => out.push_str(reference),
        }
        rest = after;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference is recognised by the STORE it names: the two Parameter Store forms and
    /// the Secrets Manager one are resolved here, and anything else is left to whoever owns
    /// it.
    #[test]
    fn a_reference_is_parsed_into_the_store_and_the_key_it_names() {
        let rows: &[(&str, Option<(Form, &str)>)] = &[
            ("{{resolve:ssm:/plain}}", Some((Form::Ssm, "/plain"))),
            (
                "{{resolve:ssm:/plain:3}}",
                Some((Form::Ssm, "/plain:3")),
            ),
            (
                "{{resolve:ssm-secure:/secret:1}}",
                Some((Form::SsmSecure, "/secret:1")),
            ),
            (
                "{{resolve:secretsmanager:a:SecretString:k}}",
                Some((Form::SecretsManager, "a:SecretString:k")),
            ),
            // A store this module resolves nothing for, and two texts that are not
            // references.
            ("{{resolve:nowhere:a}}", None),
            ("{{resolve:ssm}}", None),
            ("not a reference", None),
        ];
        for (case, expected) in rows {
            let actual = parsed(case);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Each form belongs to exactly ONE pass: the Parameter Store forms to the gate that
    /// runs before the deployment, the Secrets Manager one to the deployment itself, which
    /// is what lets a reference name a secret the same deployment creates.
    #[test]
    fn every_form_is_resolved_by_exactly_one_pass() {
        for (case, form) in [
            ("ssm", Form::Ssm),
            ("ssm-secure", Form::SsmSecure),
            ("secretsmanager", Form::SecretsManager),
        ] {
            let up_front = Pass::BeforeTheDeployment.covers(form);
            let deployed = Pass::AsTheResourceIsDeployed.covers(form);
            assert!(
                up_front != deployed,
                "case {case:?}: expected exactly one pass to cover the form, got \
                 before-the-deployment {up_front} and as-the-resource-is-deployed {deployed}"
            );
            let expected = form == Form::SecretsManager;
            assert_eq!(
                deployed, expected,
                "case {case:?}: expected as-the-resource-is-deployed {expected}, got {deployed}"
            );
        }
    }

    /// What a `secretsmanager` reference key names, field by field: the secret written as a
    /// name or as a full ARN carrying colons of its own, the optional `json-key`, and the
    /// version selector — of which a reference may state one and not both.
    #[test]
    fn a_secretsmanager_reference_names_the_secret_and_the_key_in_it() {
        let arn = "arn:aws:secretsmanager:us-east-1:000000000000:secret:broker-aB3dEf";
        let arn_with_key = format!("{arn}:SecretString:password");
        let rows: &[(&str, Result<Named<'_>, ()>)] = &[
            // The whole secret text, by name and by ARN.
            (
                "broker",
                Ok(Named {
                    secret_id: "broker",
                    json_key: None,
                    version_stage: None,
                    version_id: None,
                }),
            ),
            (
                arn,
                Ok(Named {
                    secret_id: arn,
                    json_key: None,
                    version_stage: None,
                    version_id: None,
                }),
            ),
            // One key of the document the secret holds, by name and by ARN.
            (
                "broker:SecretString:password",
                Ok(Named {
                    secret_id: "broker",
                    json_key: Some("password"),
                    version_stage: None,
                    version_id: None,
                }),
            ),
            (
                arn_with_key.as_str(),
                Ok(Named {
                    secret_id: arn,
                    json_key: Some("password"),
                    version_stage: None,
                    version_id: None,
                }),
            ),
            // The version selectors, each on its own.
            (
                "broker:SecretString:password:AWSPREVIOUS",
                Ok(Named {
                    secret_id: "broker",
                    json_key: Some("password"),
                    version_stage: Some("AWSPREVIOUS"),
                    version_id: None,
                }),
            ),
            (
                "broker:SecretString:::9f8e7d6c",
                Ok(Named {
                    secret_id: "broker",
                    json_key: None,
                    version_stage: None,
                    version_id: Some("9f8e7d6c"),
                }),
            ),
            // A reference that names no value: no secret, a secret-string field that is not
            // the one supported, both version selectors at once, and more fields than the
            // form has.
            ("", Err(())),
            ("broker:SecretBinary:password", Err(())),
            ("broker:SecretString:password:AWSCURRENT:9f8e7d6c", Err(())),
            ("broker:SecretString:password:AWSCURRENT::spare", Err(())),
        ];
        for (case, expected) in rows {
            let actual = Named::of(case).map_err(|_| ());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The secure form's value reaches the deployed document and never the recorded one,
    /// whatever else the text around it carries.
    #[test]
    fn a_secret_value_is_substituted_only_into_the_deployed_document() {
        let secure = "{{resolve:ssm-secure:/secret:1}}";
        let plain = "{{resolve:ssm:/plain}}";
        let resolutions = BTreeMap::from([
            (
                secure.to_string(),
                Resolved {
                    value: "s3cret".to_string(),
                    secret: true,
                },
            ),
            (
                plain.to_string(),
                Resolved {
                    value: "plainly".to_string(),
                    secret: false,
                },
            ),
        ]);
        let text = format!("user={plain};password={secure};tail");
        for (case, secrets, expected) in [
            (
                "the document the owning service is asked for",
                true,
                "user=plainly;password=s3cret;tail".to_string(),
            ),
            (
                "the document the stack records",
                false,
                format!("user=plainly;password={secure};tail"),
            ),
        ] {
            let actual = substituted_text(&text, &resolutions, secrets);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every reference the scan COLLECTS is one the substitution replaces, at the property
    /// path it stands at — which is what decides whether a secure one is supported there —
    /// and each pass collects only the references IT resolves.
    #[test]
    fn the_collected_references_carry_the_path_they_stand_at() {
        let document = json!({
            "Plain": "{{resolve:ssm:/a}}",
            "Nested": {"List": ["{{resolve:ssm-secure:/b:2}}", "{{resolve:secretsmanager:c}}"]},
            "Count": 3
        });
        for (case, pass, expected) in [
            (
                "the gate that runs before the deployment",
                Pass::BeforeTheDeployment,
                vec![
                    (
                        "Nested/List/0".to_string(),
                        "{{resolve:ssm-secure:/b:2}}".to_string(),
                    ),
                    ("Plain".to_string(), "{{resolve:ssm:/a}}".to_string()),
                ],
            ),
            (
                "the pass that runs as the resource is deployed",
                Pass::AsTheResourceIsDeployed,
                vec![(
                    "Nested/List/1".to_string(),
                    "{{resolve:secretsmanager:c}}".to_string(),
                )],
            ),
        ] {
            let mut collected = Vec::new();
            collect(&document, "", 0, pass, &mut collected);
            let mut actual = collected.clone();
            actual.sort();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {collected:?}"
            );
        }
    }

    /// The collection bound holds inside ONE property value too: a text carrying more
    /// references than the scan collects stops at the bound rather than growing the set to
    /// the size of the text, and the set it hands back has passed the bound the whole
    /// document is then refused on.
    #[test]
    fn one_text_cannot_grow_the_collected_set_beyond_the_bound() {
        let case = "one property value carrying four times the bound in references";
        let text = "{{resolve:ssm:/a}}".repeat(MAX_REFERENCES * 4);
        let mut collected = Vec::new();
        references_in(&text, "Property", Pass::BeforeTheDeployment, &mut collected);
        assert!(
            collected.len() > MAX_REFERENCES && collected.len() <= MAX_REFERENCES + 1,
            "case {case:?}: expected the scan to stop just past {MAX_REFERENCES}, got {}",
            collected.len()
        );
    }

    /// A secure reference is supported only at the published property paths, whatever the
    /// resource type's other properties are, and a list element's index is not part of the
    /// path.
    #[test]
    fn a_secure_reference_is_supported_only_where_aws_publishes_it() {
        let rows: &[(&str, &str, bool)] = &[
            ("AWS::RDS::DBInstance", "MasterUserPassword", true),
            ("AWS::RDS::DBInstance", "MasterUsername", false),
            // A nested path, and one through a list.
            ("AWS::IAM::User", "LoginProfile/Password", true),
            ("AWS::IAM::User", "Password", false),
            ("AWS::OpsWorks::Stack", "RdsDbInstances/0/DbPassword", true),
            // The same property name, on a type that does not publish it.
            ("AWS::SSM::Parameter", "Value", false),
            ("AWS::SNS::Topic", "MasterUserPassword", false),
        ];
        for (resource_type, path, expected) in rows {
            let actual = secure_reference_supported(resource_type, path);
            assert_eq!(
                actual, *expected,
                "case {resource_type:?}/{path:?}: expected {expected}, got {actual}"
            );
        }
    }
}
