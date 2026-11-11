// SPDX-License-Identifier: CC0-1.0

//! Writing ONE document into an index of a domain — the data-plane write a producer
//! outside this service (a Firehose stream whose destination is a domain of this
//! service, say) performs on the domain it was configured to deliver to.
//!
//! A domain of this service holds indexes ([`super::Index`], minted by `CreateIndex`),
//! and each index carries the schema the create persisted. This module is what turns a
//! producer's record into a DOCUMENT of such an index: it is the one place that decides
//! whether a write is authorized, which index it lands in, and whether the index's
//! mappings accept it, so every producer that delivers into a domain is judged alike and
//! the documents all of them write live in the one store this service reports from.
//!
//! Three decisions, in the order the cluster takes them:
//!
//! 1. WHERE — the ARN names a domain of this store, and that domain has to be provisioned
//!    (a domain still being created publishes no endpoint, so a write to it has reached
//!    nothing yet and is owed again rather than refused). An index the domain does not
//!    hold is CREATED by the write, which is what a cluster does by default
//!    (`action.auto_create_index`), with no mappings of its own.
//! 2. WHETHER — the principal the producer writes as has to be allowed to perform the
//!    data-plane write action on that index: the domain's own access policy, when it
//!    carries one, and the identity policies the IAM store holds for the principal, both
//!    read through the one shared evaluator ([`::service_common::iam_authorization`]). A
//!    write nothing allows is refused the way the engine refuses an unauthorized request,
//!    with a `security_exception`.
//! 3. WHAT — the bytes have to be one JSON object, and every field of it has to agree
//!    with the index's mappings: a field the mappings do not carry is refused where they
//!    are `strict`, and a value that is not of a mapped field's type is refused whatever
//!    the dynamic setting is.
//!
//! A refusal is reported to the CALLER rather than swallowed, because the producer is what
//! decides what happens to a document the cluster would not take — a Firehose stream, for
//! one, parks it in the destination's Amazon S3 backup.

use std::time::SystemTime;

use ::service_common::iam_authorization::{self as iam, Decision};

use super::{ComAmazonawsOpensearchAmazonopensearchserviceState, Domain, Index};

/// The IAM action a write of one document asks for: the data-plane HTTP actions of this
/// service are `es:ESHttpPost` / `es:ESHttpPut` / … ("Actions defined by Amazon OpenSearch
/// Service", IAM Service Authorization Reference), and a document write is a POST to the
/// index's document path.
pub const WRITE_ACTION: &str = "ESHttpPost";

/// The most documents ONE index of this emulator holds.
///
/// An emulator resource budget, NOT an engine limit: every document lives in this
/// process's heap, so without a cap a producer could grow the store without end. A write
/// past it is refused rather than evicting a document a reader may still be served.
pub const INDEX_DOCUMENT_QUOTA: usize = 10_000;

/// The most bytes ONE document may carry, for the same reason as [`INDEX_DOCUMENT_QUOTA`].
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// How much of a field's value a refusal quotes back, so a message naming the value a
/// field could not be parsed from is bounded by this rather than by the document.
const MAX_VALUE_PREVIEW_BYTES: usize = 64;

/// The mapping member that decides what happens to a field the mappings do not carry:
/// `strict` refuses the document, `false` accepts it without indexing the field, and
/// `true` (the default) maps the field as it arrives.
const MAPPING_DYNAMIC: &str = "dynamic";

/// The `dynamic` value that refuses a document carrying an unmapped field.
const DYNAMIC_STRICT: &str = "strict";

/// The mappings member nesting the field mappings themselves.
///
/// The sibling vocabulary this module reads — the schema's `mappings` section
/// ([`super::SCHEMA_MAPPINGS`]) and a field mapping's `type` ([`super::MAPPING_TYPE`]) —
/// is the parent module's, so there is one spelling of each; `properties` is named here on
/// its own because the parent holds it inside the SET of members a mapping nests under
/// (`MAPPING_CONTAINERS`), while a document is walked against this one alone.
const MAPPING_PROPERTIES: &str = "properties";

/// The single mapping type name a document is addressed under in this engine's
/// generation, which the refusal for an unmapped field names as the mapping the
/// introduction was attempted within.
const ROOT_MAPPING: &str = "_doc";

/// One document of an index, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct IndexedDocument {
    /// The document's id within its index — the `_id` a reader addresses it by. Unique
    /// per index, so the id IS the key the index's map holds it under.
    pub id: String,
    /// The document itself, as the `_source` a reader is answered with: the JSON object
    /// the write carried, verbatim.
    pub source: ::serde_json::Value,
    /// The instant this version of the document was written.
    pub indexed_at: SystemTime,
    /// The document's `_version`: 1 for the write that created it, one more for each
    /// later write to the same id, which is how the engine numbers a document's versions.
    pub version: u64,
    /// The document's `_seq_no`: "the sequence number assigned to the document for the
    /// indexing operation", taken from the index's own counter
    /// ([`super::Index::next_seq_no`]) by the write that last changed it. A reader feeds
    /// it back as `if_seq_no` to write only if the document has not moved since.
    pub seq_no: u64,
    /// The document's `_primary_term`: the term of the primary shard that took that
    /// write. One run of a cluster takes every write in one term and nothing here
    /// re-elects a primary, so this stays at [`PRIMARY_TERM`].
    pub primary_term: u64,
}

/// The `_primary_term` every write of this cluster is taken in.
///
/// "The primary term is a monotonically increasing number that increases each time a
/// primary shard is assigned to a different node": one run of this emulator's cluster
/// takes every write in one term, so every document reports the term the cluster started
/// in — the same reasoning the index-management plugin's own writes are numbered by.
pub const PRIMARY_TERM: u64 = 1;

/// One document to write, with everything the three decisions above read.
#[derive(Debug, Clone, Copy)]
pub struct DocumentWrite<'a> {
    /// The domain the document is written to, addressed either by its ARN or by the host
    /// of the endpoint it publishes — the two ways a producer's configuration names a
    /// domain.
    pub domain: &'a str,
    /// The index the document is written into, which the write creates if the domain
    /// does not hold it.
    pub index_name: &'a str,
    /// The document's id. A producer that mints none leaves the engine to mint one.
    pub document_id: Option<&'a str>,
    /// The document's own bytes, which have to be one JSON object.
    pub document: &'a [u8],
    /// The ARN of the principal the producer writes as — the role it assumed.
    pub principal_arn: &'a str,
    /// The instant of the write.
    pub now: SystemTime,
}

/// What a written document landed as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    /// The index it landed in.
    pub index_name: String,
    /// The id it is addressed by.
    pub document_id: String,
}

/// Why a document was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteRefusal {
    /// Nothing answered: this store holds no provisioned domain of that ARN, so the
    /// write reached no cluster at all. A producer treats it as a delivery it has not
    /// made yet rather than as a document the cluster refused.
    Unavailable {
        /// What could not be reached.
        message: String,
    },
    /// The cluster REFUSED the document: the write is not authorized, or the index's
    /// mappings do not accept it. `error_type` is the engine's own error type, the value
    /// its answer carries, and `message` its reason.
    Rejected {
        /// The engine's error type, e.g. `security_exception`.
        error_type: &'static str,
        /// The reason, as the engine words it.
        message: String,
    },
}

/// The engine's error type for a request the domain does not authorize.
pub const SECURITY_EXCEPTION: &str = "security_exception";

/// The engine's error type for an index the cluster does not hold — a read of one, or a
/// write the cluster may not create it for ([`super::auto_create_index`]).
pub const INDEX_NOT_FOUND_EXCEPTION: &str = "index_not_found_exception";

/// The engine's error type for a document carrying a field the mappings do not carry,
/// where they are `strict`.
pub const STRICT_DYNAMIC_MAPPING_EXCEPTION: &str = "strict_dynamic_mapping_exception";

/// The engine's error type for a document a field's mapped type could not parse a value
/// from, and for bytes that are not one JSON object.
pub const MAPPER_PARSING_EXCEPTION: &str = "mapper_parsing_exception";

/// The engine's error type for a write this emulator's own bounds refuse.
pub const ILLEGAL_ARGUMENT_EXCEPTION: &str = "illegal_argument_exception";

/// Write `write`'s document into the index it names, answering what it landed as or why
/// the cluster would not take it.
///
/// The ONE entry point: every producer writing into a domain of this service goes through
/// it, so the authorization decision, the auto-created index and the mapping rules are the
/// same for all of them. Takes the store's lock itself and holds it for this one write.
pub fn index_document<Eng: Send + Sync + 'static>(
    engine: &Eng,
    write: &DocumentWrite<'_>,
) -> Result<Indexed, WriteRefusal> {
    let Some(state) = super::service_state(engine) else {
        return Err(WriteRefusal::Unavailable {
            message: format!(
                "no OpenSearch Service domain is reachable at {}",
                write.domain
            ),
        });
    };
    // The identity policies are read from ANOTHER service's store, so they are read
    // before this one is locked: no call holds two service stores at once.
    let identity = principal_documents(engine, write.principal_arn);
    let mut store = state.lock();
    let domain = provisioned_domain(&mut store, write.domain, write.now)?;
    index_into(domain, write, identity.as_ref())
}

/// [`index_document`] against a domain record the caller ALREADY holds, with the identity
/// policies it already read.
///
/// The same three decisions in the same order; it is the entry point for a caller that is
/// already inside this service's store — the domain's own REST data plane
/// ([`crate::service::domain_rest`]), which resolved the domain to answer the request at
/// all — so a document written over the wire and one written by a producer land under one
/// set of rules.
pub fn index_into(
    domain: &mut Domain,
    write: &DocumentWrite<'_>,
    identity: Option<&Vec<String>>,
) -> Result<Indexed, WriteRefusal> {
    let domain_arn = domain.arn.clone();
    authorize(write, &domain_arn, domain, identity)?;
    // IAM lets the request reach the cluster; the CLUSTER decides next. On a domain with
    // fine-grained access control switched on, a principal that is neither the master
    // user nor bound by a role mapping may do nothing at all, whatever IAM allows — the
    // one decision every path into a domain takes ([`super::fgac`]).
    if let Some(reason) = super::fgac::refusal(
        domain,
        write.principal_arn,
        super::fgac::ACTION_INDEX_DOCUMENT,
    ) {
        return Err(WriteRefusal::Rejected {
            error_type: SECURITY_EXCEPTION,
            message: reason,
        });
    }
    let document = parse_document(write)?;
    // The mappings the document is judged by: an index the domain already holds judges it
    // by the ones that index carries, and an index this write would CREATE by the ones an
    // index template gives it — a template applies to an index as it is created, so a
    // document those mappings refuse is refused rather than creating the index for it.
    // With neither, nothing is mapped yet and there is nothing to disagree with.
    let mappings = domain
        .indexes
        .get(write.index_name)
        .map(|index| index.schema.clone())
        .or_else(|| domain.templated_schema(write.index_name));
    if let Some(schema) = &mappings {
        accepted_by_mappings(schema, &document, write.document_id.unwrap_or_default())?;
    }
    let index = index_for(domain, write.index_name, mappings)?;
    store_document(index, write, document)
}

/// The identity-based policy documents in force on `principal_arn`, or `None` when this
/// emulator holds none for it — a principal the IAM store does not know, or an engine
/// carrying no IAM store at all, where what the principal may do is not evidence this
/// emulator has. The same posture [`super::authorization`] takes for a grant it cannot
/// read: refusing on it would deny access AWS permits.
pub fn principal_documents<Eng: Send + Sync + 'static>(
    engine: &Eng,
    principal_arn: &str,
) -> Option<Vec<String>> {
    let documents = super::authorization::role_identity_documents(engine, principal_arn)?;
    Some(documents)
}

/// The domain `address` names — its ARN, or the host of the endpoint it publishes — with
/// every configuration change the clock already owes it materialised.
///
/// A domain that is not `Created` yet publishes no endpoint, so a write to it has reached
/// nothing: that is [`WriteRefusal::Unavailable`] and not a refusal of the document.
fn provisioned_domain<'s>(
    store: &'s mut ComAmazonawsOpensearchAmazonopensearchserviceState,
    address: &str,
    now: SystemTime,
) -> Result<&'s mut Domain, WriteRefusal> {
    let unreachable = || WriteRefusal::Unavailable {
        message: format!("no OpenSearch Service domain is reachable at {address}"),
    };
    let name = store
        .domains
        .values()
        .find(|domain| {
            domain.arn == address
                // The address a producer is CONFIGURED with is the one the control plane
                // published for the domain ([`Domain::published_endpoint`]); the host AWS
                // names that endpoint is accepted beside it, for a configuration written
                // against AWS's own name.
                || domain.published_endpoint() == address
                || domain.endpoint == address
        })
        .map(|domain| domain.domain_name.clone())
        .ok_or_else(unreachable)?;
    let domain = store.settled_mut(&name, now).ok_or_else(unreachable)?;
    if !domain.created {
        return Err(WriteRefusal::Unavailable {
            message: format!(
                "the OpenSearch Service domain at {address} is not available for requests yet"
            ),
        });
    }
    Ok(domain)
}

/// Whether the principal the producer writes as may write this document.
///
/// The decision is [`iam_decision`], the ONE reading of what IAM allows a data-plane
/// request of this service — a producer's write here and a signed request to the domain's
/// own REST surface alike — so the two cannot answer differently. A write nothing allows
/// is refused the way the engine refuses an unauthorized request, with a
/// `security_exception`.
fn authorize(
    write: &DocumentWrite<'_>,
    domain_arn: &str,
    domain: &Domain,
    identity: Option<&Vec<String>>,
) -> Result<(), WriteRefusal> {
    iam_decision(
        domain,
        write.principal_arn,
        WRITE_ACTION,
        &index_resource(domain_arn, write.index_name),
        identity,
    )
}

/// Whether IAM lets `principal_arn` perform this service's data-plane `action_name`
/// (`ESHttpPost`, `ESHttpGet`, …) on `resource` of `domain`.
///
/// The decision a document write takes ([`authorize`]) and the one the domain's REST data
/// plane takes ([`crate::service::domain_rest`]) are THIS one, so the two paths into a
/// domain cannot disagree about what IAM allows.
pub fn iam_decision(
    domain: &Domain,
    principal_arn: &str,
    action_name: &str,
    resource: &str,
    identity: Option<&Vec<String>>,
) -> Result<(), WriteRefusal> {
    let action = format!("{}:{action_name}", super::ARN_NAMESPACE);
    let principals = principal_spellings(principal_arn);
    let request = iam::Request::new(&action, resource, &principals);
    let identity_decision = identity
        .map(|documents| iam::identity_decision(documents, &request))
        .unwrap_or(Decision::NotStated);
    let access_policy = domain.access_policies.as_deref().filter(|policy| {
        // A domain carrying an empty access policy states nothing, which is the same
        // position as carrying none.
        !policy.trim().is_empty()
    });
    let resource_decision = access_policy
        .map(|policy| iam::resource_decision(policy, &request))
        .unwrap_or(Decision::NotStated);
    if iam::is_allowed(identity_decision, resource_decision) {
        return Ok(());
    }
    // The ACCOUNT'S OWN published credential is bounded by no identity policy at all — the
    // position AWS account root credentials are in, and the caller this service's
    // control-plane gate SERVES rather than evaluates
    // ([`super::authorization::is_account_credential`]) — so a domain's access policy
    // saying nothing about it does not refuse it either: IAM grants a request of this
    // account that either side allows, and for this caller the identity side is not one
    // this emulator can read. What still refuses it is the rule that wins over every allow
    // whichever side states it: an explicit `Deny`.
    if super::authorization::is_account_credential(principal_arn)
        && resource_decision != Decision::Denied
    {
        return Ok(());
    }
    // Neither side states anything and there is no policy on either side to state it: an
    // engine holding no identity store, and a domain carrying no access policy, is a domain
    // nothing bounds.
    if identity.is_none()
        && access_policy.is_none()
        && identity_decision == Decision::NotStated
        && resource_decision == Decision::NotStated
    {
        return Ok(());
    }
    Err(WriteRefusal::Rejected {
        error_type: SECURITY_EXCEPTION,
        // Worded by the one place a data-plane refusal of this service is worded
        // ([`not_authorized`]), and WHICH of IAM's rules refused follows the policy that
        // refused: an explicit `Deny` names the side it was read from, and with neither side
        // allowing, the sentence names the kind of policy that would have had to allow — the
        // domain's own access policy where it carries one, and the principal's identity
        // policies otherwise. Every value in it comes from the request and from the stored
        // policy.
        message: match (identity_decision, resource_decision) {
            (_, Decision::Denied) => not_authorized(
                principal_arn,
                &action,
                EXPLICIT_DENY_IN_A_RESOURCE_POLICY,
            ),
            (Decision::Denied, _) => not_authorized(
                principal_arn,
                &action,
                EXPLICIT_DENY_IN_AN_IDENTITY_POLICY,
            ),
            _ if access_policy.is_some() => not_authorized(
                principal_arn,
                &action,
                &format!("because no resource-based policy allows the {action} action"),
            ),
            _ => not_authorized(
                principal_arn,
                &action,
                &format!("because no identity-based policy allows the {action} action"),
            ),
        },
    })
}

/// The sentence AWS refuses a DATA-PLANE request of this service with: the principal that
/// signed, the `es:ESHttp*` action its method asks for, and which of IAM's rules refused.
///
/// It names no RESOURCE — the recorded refusal of such a request carries the principal, the
/// action and the rule and nothing else — which is what separates it from this service's
/// CONTROL-plane refusal ([`super::authorization::not_authorized`]), where the resource the
/// request named is part of the sentence. Every value in it comes from the request and from
/// the stored policies.
fn not_authorized(principal_arn: &str, action: &str, reason: &str) -> String {
    format!("User: {principal_arn} is not authorized to perform: {action} {reason}")
}

/// The clause IAM refuses a request with that a RESOURCE-based policy explicitly denies —
/// the rule that wins over every allow, whichever side stated it.
const EXPLICIT_DENY_IN_A_RESOURCE_POLICY: &str =
    "with an explicit deny in a resource-based policy";

/// The same clause for an explicit `Deny` read from the principal's own identity policies.
const EXPLICIT_DENY_IN_AN_IDENTITY_POLICY: &str =
    "with an explicit deny in an identity-based policy";

/// The resource an index's data-plane write is decided against: the index under its
/// domain's ARN, which is how an IAM policy for this service names one
/// (`arn:<partition>:es:<region>:<account>:domain/<domain>/<index>/…`).
fn index_resource(domain_arn: &str, index_name: &str) -> String {
    format!("{domain_arn}/{index_name}")
}

/// The spellings a resource policy's `Principal` element names this principal by: the
/// identities the principal IS — its own ARN, and for an assumed-role session the IAM role
/// it is a session of, the one reading of that pair this crate has
/// ([`super::fgac::identities`]) — and the account spellings a statement about the whole
/// account uses.
///
/// A resource policy is written with the identity a client CREATED, which for a role is the
/// role's own ARN rather than any one session of it, so a statement naming a role is about
/// every session of that role: without the role spelling, a policy that explicitly denies a
/// role would state nothing about the requests that role actually makes.
fn principal_spellings(principal_arn: &str) -> Vec<String> {
    let mut spellings = super::fgac::identities(principal_arn);
    spellings.extend(iam::account_principals(principal_arn));
    spellings
}

/// The index `index_name` names on `domain`, CREATED with `mappings` if the domain does
/// not hold it — which is what a cluster does for a write to an index that does not exist
/// yet (`action.auto_create_index`), carrying the schema an index template gives it and
/// none where no template matches.
///
/// Creating one is bounded by the same per-domain quota a `CreateIndex` call is held to
/// ([`super::INDEX_QUOTA`]): a producer writing to a fresh index name per record must not
/// grow this store without end, so past the quota the write is refused rather than adding
/// another index.
fn index_for<'d>(
    domain: &'d mut Domain,
    index_name: &str,
    mappings: Option<::aws_smithy_types::Document>,
) -> Result<&'d mut Index, WriteRefusal> {
    if !domain.indexes.contains_key(index_name) {
        // A write to an index the cluster does not hold CREATES it only while
        // `action.auto_create_index` lets it: the setting is what a client turns implicit
        // creation off with, and a write it forbids is refused rather than minting the
        // index ([`super::auto_create_index`]).
        if let super::AutoCreate::Refused(reason) = domain.auto_creates_index(index_name) {
            return Err(WriteRefusal::Rejected {
                error_type: INDEX_NOT_FOUND_EXCEPTION,
                message: reason,
            });
        }
        if domain.indexes.len() >= super::INDEX_QUOTA {
            return Err(WriteRefusal::Rejected {
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                message: format!(
                    "domain [{}] already holds the maximum of {} indexes, so index \
                     [{index_name}] could not be created",
                    domain.domain_name,
                    super::INDEX_QUOTA
                ),
            });
        }
    }
    Ok(domain
        .indexes
        .entry(index_name.to_string())
        .or_insert_with(|| Index {
            index_name: index_name.to_string(),
            aliases: ::std::collections::BTreeMap::new(),
            next_seq_no: 0,
            // Nothing named this index's mappings unless a template did, in which case
            // the index is created carrying them, exactly as a `CreateIndex` with that
            // schema would have left it.
            schema: mappings.unwrap_or_else(|| {
                ::aws_smithy_types::Document::Object(::std::collections::HashMap::new())
            }),
            documents: ::std::collections::BTreeMap::new(),
        }))
}

/// The document's bytes as the JSON object they have to be.
fn parse_document(write: &DocumentWrite<'_>) -> Result<::serde_json::Value, WriteRefusal> {
    if write.document.len() > MAX_DOCUMENT_BYTES {
        return Err(WriteRefusal::Rejected {
            error_type: ILLEGAL_ARGUMENT_EXCEPTION,
            message: format!(
                "document is larger than the maximum of {MAX_DOCUMENT_BYTES} bytes this \
                 index takes"
            ),
        });
    }
    let parsed: ::serde_json::Value =
        ::serde_json::from_slice(write.document).map_err(|error| WriteRefusal::Rejected {
            error_type: MAPPER_PARSING_EXCEPTION,
            message: format!("failed to parse: {error}"),
        })?;
    if !parsed.is_object() {
        return Err(WriteRefusal::Rejected {
            error_type: MAPPER_PARSING_EXCEPTION,
            message: "failed to parse: expected a document to be a JSON object".to_string(),
        });
    }
    Ok(parsed)
}

/// Store the document, answering what it landed as.
fn store_document(
    index: &mut Index,
    write: &DocumentWrite<'_>,
    source: ::serde_json::Value,
) -> Result<Indexed, WriteRefusal> {
    let id = match write.document_id {
        Some(id) if !id.is_empty() => id.to_string(),
        // A write that names no id leaves the engine to mint one, which is what a POST to
        // an index's document path does.
        _ => ::uuid::Uuid::new_v4().to_string(),
    };
    let existing = index.documents.get(&id).map(|document| document.version);
    if existing.is_none() && index.documents.len() >= INDEX_DOCUMENT_QUOTA {
        return Err(WriteRefusal::Rejected {
            error_type: ILLEGAL_ARGUMENT_EXCEPTION,
            message: format!(
                "index [{}] already holds the maximum of {INDEX_DOCUMENT_QUOTA} documents",
                index.index_name
            ),
        });
    }
    // Every write takes the next sequence number of the index it lands in, whether it
    // created the document or replaced one, which is what makes the pair a reader feeds
    // back as `if_seq_no`/`if_primary_term` move with each write.
    let seq_no = index.take_seq_no();
    index.documents.insert(
        id.clone(),
        IndexedDocument {
            id: id.clone(),
            source,
            indexed_at: write.now,
            version: existing.unwrap_or(0).saturating_add(1),
            seq_no,
            primary_term: PRIMARY_TERM,
        },
    );
    Ok(Indexed {
        index_name: index.index_name.clone(),
        document_id: id,
    })
}

/// Whether `document` agrees with the field mappings `schema` carries.
///
/// The walk descends both together, field by field: a field the mappings carry is checked
/// against its mapped type, a field they do not carry is refused where that level of the
/// mappings is `strict`, and an object field's own `properties` are walked the same way.
/// Iterative rather than recursive, so a deeply nested document costs no stack.
fn accepted_by_mappings(
    schema: &::aws_smithy_types::Document,
    document: &::serde_json::Value,
    document_id: &str,
) -> Result<(), WriteRefusal> {
    use ::aws_smithy_types::Document;

    let mappings = match schema {
        Document::Object(root) => root.get(super::SCHEMA_MAPPINGS),
        _ => None,
    };
    // Nothing named this index's mappings, so every field of the document is dynamically
    // mapped and there is no mapped type to disagree with.
    let Some(mappings) = mappings else {
        return Ok(());
    };
    let mut pending = vec![(String::new(), mappings, document)];
    while let Some((path, mappings, value)) = pending.pop() {
        let Document::Object(mappings) = mappings else {
            continue;
        };
        let ::serde_json::Value::Object(fields) = value else {
            continue;
        };
        let properties = match mappings.get(MAPPING_PROPERTIES) {
            Some(Document::Object(properties)) => Some(properties),
            _ => None,
        };
        let strict = matches!(
            mappings.get(MAPPING_DYNAMIC),
            Some(Document::String(dynamic)) if dynamic == DYNAMIC_STRICT
        );
        for (name, field) in fields {
            let field_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            let mapping = properties.and_then(|properties| properties.get(name.as_str()));
            let Some(mapping) = mapping else {
                if strict && !field.is_null() {
                    let within = if path.is_empty() {
                        ROOT_MAPPING.to_string()
                    } else {
                        path.clone()
                    };
                    return Err(WriteRefusal::Rejected {
                        error_type: STRICT_DYNAMIC_MAPPING_EXCEPTION,
                        message: format!(
                            "mapping set to strict, dynamic introduction of [{name}] within \
                             [{within}] is not allowed"
                        ),
                    });
                }
                continue;
            };
            // A mapping that is not a set of members names no type and nests no field,
            // so it says nothing about the value standing at that field.
            let Document::Object(members) = mapping else {
                continue;
            };
            let mapped_type = match members.get(super::MAPPING_TYPE) {
                Some(Document::String(name)) => Some(name.as_str()),
                // A mapping that nests `properties` and names no type IS an object
                // field: a field holding further fields is mapped as an object.
                _ => members.get(MAPPING_PROPERTIES).map(|_| TYPE_OBJECT),
            };
            if let Some(mapped_type) = mapped_type {
                if !parses_as(mapped_type, field) {
                    return Err(WriteRefusal::Rejected {
                        error_type: MAPPER_PARSING_EXCEPTION,
                        message: format!(
                            "failed to parse field [{field_path}] of type [{mapped_type}] in \
                             document with id '{document_id}'. Preview of field's value: \
                             '{}'",
                            value_preview(field)
                        ),
                    });
                }
            }
            // An object field's own fields are checked against the mapping nested under
            // it, so a strict object refuses an unmapped sub-field of its own.
            if field.is_object() {
                pending.push((field_path, mapping, field));
            }
        }
    }
    Ok(())
}

/// The field type an object field carries when its mapping nests further fields and names
/// no type of its own.
const TYPE_OBJECT: &str = "object";

/// Whether a value of a field mapped with `mapped_type` can be parsed from `value`.
///
/// The catalog is the engine's own documented FIELD TYPES (OpenSearch "Supported field
/// types"), grouped by what each parses:
///
/// * a string field (`text`, `keyword`, …) takes any scalar, since a number or a boolean
///   is converted to its string form, and no object;
/// * a numeric field takes a number, and a string a number can be read from — numeric
///   fields coerce by default ("coerce: try to convert strings to numbers");
/// * a boolean field takes `true`/`false` and the strings spelling them, and no number;
/// * a date field takes epoch milliseconds or a date-formatted string;
/// * an `ip` field takes a string holding an IP address;
/// * an object field takes an object;
/// * any other type — a geo shape, a vector, a binary blob — is one this emulator does
///   not judge, so a value of it is taken as it arrives.
///
/// `null` is taken by every type: a null field is simply not indexed. An ARRAY is taken
/// where each of its elements is, because "there is no dedicated array type: any field can
/// contain zero or more values of the same type".
fn parses_as(mapped_type: &str, value: &::serde_json::Value) -> bool {
    use ::serde_json::Value;

    match value {
        Value::Null => return true,
        Value::Array(elements) => {
            return elements
                .iter()
                .all(|element| parses_as(mapped_type, element))
        }
        _ => {}
    }
    match mapped_type {
        "text" | "keyword" | "match_only_text" | "wildcard" | "constant_keyword" | "search_as_you_type" => {
            !value.is_object()
        }
        "long" | "integer" | "short" | "byte" | "double" | "float" | "half_float"
        | "scaled_float" | "unsigned_long" => match value {
            Value::Number(_) => true,
            Value::String(text) => text.trim().parse::<f64>().is_ok(),
            _ => false,
        },
        "boolean" => match value {
            Value::Bool(_) => true,
            Value::String(text) => matches!(text.as_str(), "true" | "false" | ""),
            _ => false,
        },
        "date" | "date_nanos" => match value {
            Value::Number(_) => true,
            Value::String(text) => parses_as_date(text),
            _ => false,
        },
        "ip" => match value {
            Value::String(text) => text.parse::<::std::net::IpAddr>().is_ok(),
            _ => false,
        },
        TYPE_OBJECT | "nested" | "flat_object" => value.is_object(),
        _ => true,
    }
}

/// Whether `text` holds a date a date field's default format reads: the engine's
/// `strict_date_optional_time||epoch_millis`, so an ISO 8601 instant or a count of epoch
/// milliseconds.
///
/// The instant is parsed by the runtime's own reader of that format rather than by hand.
fn parses_as_date(text: &str) -> bool {
    if text.trim().parse::<i64>().is_ok() {
        return true;
    }
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .is_ok()
}

/// How much of `value` a refusal quotes, bounded by [`MAX_VALUE_PREVIEW_BYTES`] so the
/// message is sized by that and not by the document.
fn value_preview(value: &::serde_json::Value) -> String {
    let rendered = match value {
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if rendered.len() <= MAX_VALUE_PREVIEW_BYTES {
        return rendered;
    }
    rendered
        .char_indices()
        .take_while(|(at, _)| *at < MAX_VALUE_PREVIEW_BYTES)
        .map(|(_, character)| character)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema document from its JSON text, the way a `CreateIndex` request carries one.
    fn schema(text: &str) -> ::aws_smithy_types::Document {
        let parsed: ::serde_json::Value = ::serde_json::from_str(text).expect("the schema parses");
        document_of(&parsed)
    }

    /// A Smithy document mirroring a JSON value.
    fn document_of(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
        use ::aws_smithy_types::{Document, Number};
        match value {
            ::serde_json::Value::Null => Document::Null,
            ::serde_json::Value::Bool(flag) => Document::Bool(*flag),
            ::serde_json::Value::String(text) => Document::String(text.clone()),
            ::serde_json::Value::Number(number) => Document::Number(
                number
                    .as_i64()
                    .map(Number::NegInt)
                    .unwrap_or_else(|| Number::Float(number.as_f64().unwrap_or_default())),
            ),
            ::serde_json::Value::Array(elements) => {
                Document::Array(elements.iter().map(document_of).collect())
            }
            ::serde_json::Value::Object(members) => Document::Object(
                members
                    .iter()
                    .map(|(name, member)| (name.clone(), document_of(member)))
                    .collect(),
            ),
        }
    }

    /// RULE: a `strict` mapping refuses a document carrying a field it does not map, and
    /// takes one whose fields it all maps.
    #[test]
    fn a_strict_mapping_refuses_an_unmapped_field() {
        let strict = schema(
            r#"{"mappings":{"dynamic":"strict","properties":{"level":{"type":"keyword"}}}}"#,
        );
        let accepted = ::serde_json::json!({ "level": "info" });
        assert!(
            accepted_by_mappings(&strict, &accepted, "").is_ok(),
            "case \"a mapped field\": expected the document to be taken"
        );
        let refused = ::serde_json::json!({ "level": "info", "extra": 1 });
        let answer = accepted_by_mappings(&strict, &refused, "");
        assert!(
            matches!(
                &answer,
                Err(WriteRefusal::Rejected { error_type, message })
                    if *error_type == STRICT_DYNAMIC_MAPPING_EXCEPTION
                        && message.contains("[extra]")
                        && message.contains(ROOT_MAPPING)
            ),
            "case \"an unmapped field\": expected a strict refusal naming the field, got \
             {answer:?}"
        );
    }

    /// RULE: without `strict`, an unmapped field is taken — the default dynamic mapping.
    #[test]
    fn a_dynamic_mapping_takes_an_unmapped_field() {
        let dynamic = schema(r#"{"mappings":{"properties":{"level":{"type":"keyword"}}}}"#);
        let document = ::serde_json::json!({ "level": "info", "extra": 1 });
        assert!(
            accepted_by_mappings(&dynamic, &document, "").is_ok(),
            "case \"a dynamic mapping\": expected the unmapped field to be taken"
        );
    }

    /// RULE: a mapped field takes the values its type can be parsed from and refuses the
    /// rest, whatever the dynamic setting is. The rows span the engine's field-type
    /// families.
    #[test]
    fn a_mapped_field_refuses_a_value_its_type_cannot_parse() {
        // (the mapped type, a value it takes, a value it refuses)
        let rows: &[(&str, ::serde_json::Value, ::serde_json::Value)] = &[
            (
                "keyword",
                ::serde_json::json!("info"),
                ::serde_json::json!({ "nested": true }),
            ),
            ("long", ::serde_json::json!(7), ::serde_json::json!("seven")),
            (
                "double",
                ::serde_json::json!("1.5"),
                ::serde_json::json!(true),
            ),
            (
                "boolean",
                ::serde_json::json!("true"),
                ::serde_json::json!(1),
            ),
            (
                "date",
                ::serde_json::json!("2026-01-02T03:04:05Z"),
                ::serde_json::json!("not-a-date"),
            ),
            (
                "ip",
                ::serde_json::json!("10.0.0.1"),
                ::serde_json::json!("10.0.0.256"),
            ),
            (
                "object",
                ::serde_json::json!({ "inner": 1 }),
                ::serde_json::json!("flat"),
            ),
        ];
        for (mapped_type, accepted, refused) in rows {
            let mappings = schema(&format!(
                r#"{{"mappings":{{"properties":{{"field":{{"type":"{mapped_type}"}}}}}}}}"#
            ));
            let taken = accepted_by_mappings(
                &mappings,
                &::serde_json::json!({ "field": accepted }),
                "doc-1",
            );
            assert!(
                taken.is_ok(),
                "case {mapped_type:?}: expected {accepted} to be taken, got {taken:?}"
            );
            let answer = accepted_by_mappings(
                &mappings,
                &::serde_json::json!({ "field": refused }),
                "doc-1",
            );
            assert!(
                matches!(
                    &answer,
                    Err(WriteRefusal::Rejected { error_type, message })
                        if *error_type == MAPPER_PARSING_EXCEPTION
                            && message.contains("[field]")
                            && message.contains(mapped_type)
                ),
                "case {mapped_type:?}: expected {refused} to be refused as a parse failure, \
                 got {answer:?}"
            );
            // A null and an array of takeable values are taken by every type.
            for (label, value) in [
                ("null", ::serde_json::json!(null)),
                ("an array", ::serde_json::json!([accepted, accepted])),
            ] {
                let answer = accepted_by_mappings(
                    &mappings,
                    &::serde_json::json!({ "field": value }),
                    "doc-1",
                );
                assert!(
                    answer.is_ok(),
                    "case {mapped_type:?} with {label}: expected it to be taken, got {answer:?}"
                );
            }
        }
    }

    /// RULE: a value a refusal quotes is bounded, so the message is sized by the bound
    /// and not by the document.
    #[test]
    fn a_quoted_value_is_bounded() {
        let long = "x".repeat(MAX_VALUE_PREVIEW_BYTES * 4);
        let quoted = value_preview(&::serde_json::Value::String(long));
        assert!(
            quoted.len() <= MAX_VALUE_PREVIEW_BYTES,
            "case \"a long value\": expected at most {MAX_VALUE_PREVIEW_BYTES} bytes, got {}",
            quoted.len()
        );
    }
}
