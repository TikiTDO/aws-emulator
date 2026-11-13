// SPDX-License-Identifier: CC0-1.0

//! How this service puts a persisted [`App`], [`Branch`] or [`DomainAssociation`] on
//! the wire.
//!
//! Amplify answers with a COMPLETE record for an app and a branch. Three properties
//! of that record cannot be expressed by the generated serializer alone, and all
//! three are what this module adds:
//!
//! * **Every modelled member reaches the wire.** The generated serializer skips an
//!   `Option` member that holds `None`, so `Branch$buildSpec`,
//!   `Branch$sourceBranch`, `App$productionBranch` — every optional member a record
//!   has no value for — would be absent from the body, while Amplify sends the key
//!   with JSON `null`. A client dereferencing such a member gets `undefined` instead
//!   of `null`, and a list element ends up a different SHAPE from the same record
//!   fetched on its own. The projection therefore writes JSON `null` for every
//!   modelled member the record itself did not write, and completes the nested
//!   `Branch$backend` the same way (`{"stackArn": null}`). A structure the CALLER
//!   configures is the one exception — it is echoed with the members that
//!   configuration carries; see [`nested_shape`].
//! * **A member the service has no value for is JSON `null`.** `App$description`,
//!   `App$repository`, `App$environmentVariables`, `Branch$description`,
//!   `Branch$activeJobId`, `Branch$customDomains`, `Branch$environmentVariables` and
//!   `Branch$framework` are `@required` members
//!   (`smithy_query(query="shape",shape="App")` / `shape="Branch"`) whose `length`
//!   minimum is 0, so the generated record type holds each as a bare `String`,
//!   `Vec` or `HashMap` and the generated serializer writes that type's zero value —
//!   `""`, `[]`, `{}`. A client reading `""` where Amplify sends `null` draws the
//!   opposite conclusion from an emptiness test, so the projection writes the zero
//!   value of those members as `null` instead.
//! * **A read reports the resource's tag map.** `tags` is optional on both shapes,
//!   and a reporting operation answers with the map the resource carries — the empty
//!   map when it carries none — while the operation that wrote the record echoes only
//!   the tags the request set.
//!
//! Everything else about the walk stays generated: the projection wraps the record's
//! own [`SerializableStruct`] impl rather than re-listing its members, and the member
//! list it completes against is the record's own [`Schema`], so a member added to the
//! model reaches the wire without touching this file.
//!
//! Every operation that answers with an app or a branch — single record or list
//! element — answers through these two types, so a `ListBranches` element and a
//! `GetBranch` body cannot disagree about the same branch.
//!
//! A DOMAIN ASSOCIATION is the opposite case, and [`DomainAssociationProjection`] is
//! what it answers through: it is provisioned asynchronously, so its record is
//! incomplete while that provisioning runs and Amplify sends only the members it
//! already has a value for. That projection therefore DROPS a member with no value
//! instead of completing it.
//!
//! A JOB SUMMARY is the same case as a domain association, and
//! [`JobSummaryProjection`] is what it answers through: a job whose work nothing has
//! begun has no commit, no start time and no type to report, so those members are left
//! out rather than sent as the zero value of their type.
//!
//! [`ValuedMembers`] is that dropping half on its own, for an ANSWER that leaves a
//! member out: an operation whose output carries one member or another depending on
//! the form of the request writes its result through it (see
//! [`CreateDeploymentResponse`](super::CreateDeploymentResponse)), naming the members
//! this answer does not carry and whether each is dropped only when it has no value
//! ([`Unsent`]).

use super::*;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::DeserializableStruct;
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// An [`App`] as every operation that returns one writes it.
#[derive(Debug, Clone)]
pub struct AppProjection(App);

impl AppProjection {
    /// The shape this projects is still `com.amazonaws.amplify#App`: the projection
    /// changes how a record is WRITTEN, never which shape it is. The generated
    /// serializer for a list of apps reads the element schema from this constant.
    pub const SCHEMA: Schema = App::SCHEMA;

    /// The app as the operation that WROTE the record answers it: the record exactly
    /// as stored, `tags` included only when the request that created it set them.
    pub fn written(app: App) -> Self {
        Self(app)
    }

    /// The app as a REPORTING operation (`GetApp`, `ListApps`) answers it: the stored
    /// record, plus the tag map every read carries — the empty map when the app has
    /// no tags.
    ///
    /// This fills the member on the OUTGOING copy only; the stored record is
    /// untouched, so a read changes nothing a later caller sees.
    pub fn reported(mut app: App) -> Self {
        app.tags = Some(app.tags.unwrap_or_default());
        Self(app)
    }

    /// The members this record has no value for, which Amplify reports as `null`.
    ///
    /// Each is `@required` in the model, so "no value" can only be held as the zero
    /// value of the member's type; reporting that zero value verbatim is what this
    /// projection exists to prevent. `platform`, `name`, `appId`, `appArn`,
    /// `defaultDomain` and the three `@required` booleans are NOT listed: the service
    /// determines a value for every one of them.
    fn unvalued_members(&self) -> Vec<&'static str> {
        let mut unvalued = Vec::new();
        if self.0.description.is_empty() {
            unvalued.push("description");
        }
        if self.0.repository.is_empty() {
            unvalued.push("repository");
        }
        if self.0.environment_variables.is_empty() {
            unvalued.push("environmentVariables");
        }
        unvalued
    }
}

impl SerializableStruct for AppProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let unvalued = self.unvalued_members();
        CompleteRecord {
            record: &self.0,
            shape: APP_SHAPE,
            unvalued: &unvalued,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for AppProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self(<App as DeserializableStruct>::deserialize(
            deserializer,
        )?))
    }
}

/// A [`Branch`] as every operation that returns one writes it.
#[derive(Debug, Clone)]
pub struct BranchProjection(Branch);

impl BranchProjection {
    /// The shape this projects is still `com.amazonaws.amplify#Branch`; see
    /// [`AppProjection::SCHEMA`].
    pub const SCHEMA: Schema = Branch::SCHEMA;

    /// The branch as the operation that WROTE the record answers it.
    pub fn written(branch: Branch) -> Self {
        Self(branch)
    }

    /// The branch as a REPORTING operation (`GetBranch`, `ListBranches`) or an
    /// UPDATE (`UpdateBranch`) answers it: the stored record plus the tag map every
    /// one of those answers carries. The stored record is untouched.
    pub fn reported(mut branch: Branch) -> Self {
        branch.tags = Some(branch.tags.unwrap_or_default());
        Self(branch)
    }

    /// The members this record has no value for, which Amplify reports as `null`.
    ///
    /// Each is `@required` in the model, so "no value" can only be held as the zero
    /// value of the member's type; reporting that zero value verbatim is what this
    /// projection exists to prevent. `stage`, `displayName`, `ttl`,
    /// `totalNumberOfJobs` and the `@required` booleans are NOT listed: the service
    /// determines a value for every one of them.
    fn unvalued_members(&self) -> Vec<&'static str> {
        let mut unvalued = Vec::new();
        if self.0.description.is_empty() {
            unvalued.push("description");
        }
        if self.0.active_job_id.is_empty() {
            unvalued.push("activeJobId");
        }
        if self.0.custom_domains.is_empty() {
            unvalued.push("customDomains");
        }
        if self.0.environment_variables.is_empty() {
            unvalued.push("environmentVariables");
        }
        if self.0.framework.is_empty() {
            unvalued.push("framework");
        }
        unvalued
    }
}

impl SerializableStruct for BranchProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let unvalued = self.unvalued_members();
        CompleteRecord {
            record: &self.0,
            shape: BRANCH_SHAPE,
            unvalued: &unvalued,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for BranchProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self(<Branch as DeserializableStruct>::deserialize(
            deserializer,
        )?))
    }
}

/// A [`DomainAssociation`] as every operation that returns one writes it.
///
/// A domain association is provisioned ASYNCHRONOUSLY, so unlike an app or a branch
/// its record is INCOMPLETE until the provisioning ends: Amplify sends only the
/// members it already has a value for, and a member it does not is absent from the
/// body rather than present and empty. The generated serializer skips an `Option`
/// member holding `None` on its own; this projection extends that to the `@required`
/// members whose "no value" can only be the empty string, wherever they sit in the
/// record.
///
/// Which members those are depends on what the operation ANSWERS, exactly as it does
/// for an app or a branch: the operation that wrote the association echoes the
/// subdomain settings the request configured, while a REPORTING operation answers
/// the service's own view of each subdomain. See [`Self::written`] and
/// [`Self::reported`].
#[derive(Debug, Clone)]
pub struct DomainAssociationProjection {
    /// The association record, as stored.
    record: DomainAssociation,
    /// The members this answer leaves out when they have no value.
    unsent: &'static [&'static str],
}

impl DomainAssociationProjection {
    /// The shape this projects is still `com.amazonaws.amplify#DomainAssociation`;
    /// see [`AppProjection::SCHEMA`].
    pub const SCHEMA: Schema = DomainAssociation::SCHEMA;

    /// The association as the operation that WROTE the record answers it
    /// (`CreateDomainAssociation`): the service has stated no reason for a status it
    /// has nothing to add about, so `statusReason` is left out — but the subdomain
    /// settings are the CALLER's configuration, echoed with the members that
    /// configuration carries, including a `prefix` the request stated as empty.
    pub fn written(association: DomainAssociation) -> Self {
        Self {
            record: association,
            unsent: UNSENT_WHEN_WRITTEN,
        }
    }

    /// The association as a REPORTING operation answers it (`GetDomainAssociation`,
    /// `ListDomainAssociations`): the service's own view of the association, which
    /// carries a member only where it has a value for it — so the subdomain that IS
    /// the domain reports no prefix.
    pub fn reported(association: DomainAssociation) -> Self {
        Self {
            record: association,
            unsent: UNSENT_WHEN_REPORTED,
        }
    }
}

impl SerializableStruct for DomainAssociationProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        ValuedMembers {
            record: &self.record,
            unsent: self.unsent,
            // A domain association reports a member wherever it HAS a value for it:
            // which members it has values for changes as its provisioning runs.
            when: Unsent::WithoutValue,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for DomainAssociationProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self::reported(<DomainAssociation as DeserializableStruct>::deserialize(
            deserializer,
        )?))
    }
}

/// A [`JobSummary`] as every operation that answers one writes it: the record, minus
/// the members the job HAS no value for.
///
/// Amplify reports a member of a job summary only where the job has something to report
/// for it, and a job whose work nothing has begun has nothing to report for most of
/// them: a deployment that was only reserved has run no commit of a repository, has not
/// started, and has not been told what kind of job will run under its id. Those members
/// are `@required` with a `length` minimum of 0, or timestamps
/// (`smithy_query(query="shape",shape="JobSummary")`), so the generated record holds each
/// as a bare `String` or `DateTime` and the generated serializer writes that type's zero
/// value — an empty commit id, an invented commit instant — where Amplify writes no key
/// at all. A client cannot tell "no commit information" from "an empty commit id", so a
/// manual deployment would be read as a repository build with a blank commit; this
/// projection leaves those members out instead.
///
/// The record itself is what the service STORES for the job, so one job cannot be
/// reported one way by `GetJob` and another by `ListJobs`, `StopJob` or `DeleteJob`:
/// which members it has values for is decided once, where the job record is created.
#[derive(Debug, Clone)]
pub struct JobSummaryProjection {
    /// The summary record, as stored.
    summary: JobSummary,
    /// The members this job has no value for, by their modelled names.
    unvalued: &'static [&'static str],
}

impl JobSummaryProjection {
    /// The shape this projects is still `com.amazonaws.amplify#JobSummary`; see
    /// [`AppProjection::SCHEMA`].
    pub const SCHEMA: Schema = JobSummary::SCHEMA;

    /// The summary of a job whose work a start call is RUNNING: the service determined
    /// every member of it when it started the job, so the whole record reaches the wire.
    pub fn executed(summary: JobSummary) -> Self {
        Self {
            summary,
            unvalued: &[],
        }
    }

    /// The summary of a job a `CreateDeployment` only RESERVED: nothing has run under it,
    /// so it reports neither commit information, nor a start or end time, nor the type of
    /// job that will run — see [`UNVALUED_ON_A_JOB_NOTHING_RAN`].
    pub fn reserved(summary: JobSummary) -> Self {
        Self {
            summary,
            unvalued: UNVALUED_ON_A_JOB_NOTHING_RAN,
        }
    }
}

/// The stored record, so the service reads and writes the job's own members — the
/// status the clock settles, the end instant — through the projection that carries it.
impl ::std::ops::Deref for JobSummaryProjection {
    type Target = JobSummary;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

impl ::std::ops::DerefMut for JobSummaryProjection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.summary
    }
}

impl SerializableStruct for JobSummaryProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        ValuedMembers {
            record: &self.summary,
            unsent: self.unvalued,
            // Which members the job has no value for is the RECORD's property, decided
            // where the job was created, not something re-read from the values: the
            // instant a reserved job was reserved is stored (a timestamp has no empty
            // value) and still not reported.
            when: Unsent::Always,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for JobSummaryProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self::executed(<JobSummary as DeserializableStruct>::deserialize(deserializer)?))
    }
}

/// The members a job NOTHING HAS RUN has no value for, and so leaves out of every answer
/// carrying it.
///
/// `commitId`, `commitMessage` and `commitTime` describe "the commit ... from a
/// third-party repository provider for the job", which a manually deployed app has none
/// of; `startTime` is "the start date and time for the job", which has not started; and
/// `jobType` names what kind of job ran — `MANUAL`, `RELEASE`, `RETRY` or `WEB_HOOK`
/// (`smithy_query(query="shape",shape="JobSummary")`) — which is settled by the call
/// that starts the job, not by the one that reserved its id. `endTime`, `sourceUrl` and
/// `sourceUrlType` are modelled optional, so the record holds no value for them at all
/// and the generated serializer already leaves them out.
pub const UNVALUED_ON_A_JOB_NOTHING_RAN: &[&str] = &[
    "commitId",
    "commitMessage",
    "commitTime",
    "startTime",
    "jobType",
];

/// The member a domain-association record never carries without a value, whichever
/// operation answers it.
///
/// `DomainAssociation$statusReason` ("additional information that describes why the
/// domain association is in the current state") is `@required` with a `length`
/// minimum of 0 (`smithy_query(query="shape",shape="DomainAssociation")`), so the
/// generated record holds it as a bare `String` and the generated serializer writes
/// that type's zero value, `""`. A client that tests whether the member is PRESENT —
/// to decide whether the service has said anything about the association's state —
/// draws the opposite conclusion from `""`, so a reason the service does not have is
/// dropped from the body.
const UNSENT_WHEN_WRITTEN: &[&str] = &["statusReason"];

/// The members a REPORTED domain association leaves out: the one above, plus
/// `SubDomainSetting$prefix` ("the prefix setting for the subdomain", `@required`,
/// `length` minimum 0 — `smithy_query(query="shape",shape="SubDomainSetting")`),
/// which the service reports only for a subdomain that HAS a prefix: the empty
/// prefix is the apex, the associated domain itself.
const UNSENT_WHEN_REPORTED: &[&str] = &["statusReason", "prefix"];

/// WHEN a member named in an answer's `unsent` list is left out of the body.
///
/// Both halves are the same rule — Amplify does not send a member this answer has
/// nothing to report for — and they differ only in what "nothing to report" is: a
/// record whose member holds no value, or an answer the member is not part of at all.
#[derive(Debug, Clone, Copy)]
pub enum Unsent {
    /// Left out only where the member HAS no value: the empty string a `@required`
    /// member whose `length` minimum is 0 holds for "nothing". The same member is sent
    /// wherever it does have one, which is why the decision reads the value and not
    /// only the member name.
    WithoutValue,
    /// Left out whatever it holds, because the answer being written does not carry the
    /// member: the request's form decided that, not the value.
    Always,
}

/// `record`, written with every member of `unsent` this answer does not carry left
/// out — at any depth, since the members concerned sit both on the record and inside
/// the structures its lists carry.
pub struct ValuedMembers<'r> {
    /// The record to write.
    pub record: &'r dyn SerializableStruct,
    /// The members this answer may leave out, by their modelled names.
    pub unsent: &'static [&'static str],
    /// When each of them is left out.
    pub when: Unsent,
}

impl SerializableStruct for ValuedMembers<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        self.record.serialize_members(&mut Valued {
            inner: serializer,
            unsent: self.unsent,
            when: self.when,
        })
    }
}

/// A [`ShapeSerializer`] that DROPS the write of a member this answer does not carry
/// and passes every other write through, re-wrapping the structures, lists and maps
/// below it so one rule holds for the whole record.
struct Valued<'s> {
    inner: &'s mut dyn ShapeSerializer,
    unsent: &'static [&'static str],
    when: Unsent,
}

impl Valued<'_> {
    /// Is this write one Amplify does not send? `value` is the string being written,
    /// for the one case decided from the VALUE as well as the member — because the
    /// same member, a subdomain's `prefix`, is sent for one element of a list and not
    /// for another — and `None` for a write of any other type.
    fn unsent(&self, schema: &Schema, value: Option<&str>) -> bool {
        let named = schema
            .member_name()
            .is_some_and(|name| self.unsent.contains(&name));
        named
            && match self.when {
                Unsent::Always => true,
                Unsent::WithoutValue => value.is_some_and(str::is_empty),
            }
    }
}

/// Emit the [`ShapeSerializer`] methods that write a member of a type this projection
/// drops only for an answer that does not carry it at all: the write is dropped when
/// the member is one of those, and goes straight to the inner serializer otherwise.
macro_rules! pass_through {
    ($( fn $method:ident($($arg:ident: $ty:ty),*); )*) => {
        $(
            fn $method(
                &mut self,
                schema: &Schema,
                $($arg: $ty),*
            ) -> ::std::result::Result<(), SerdeError> {
                if self.unsent(schema, None) {
                    return Ok(());
                }
                self.inner.$method(schema, $($arg),*)
            }
        )*
    };
}

impl ShapeSerializer for Valued<'_> {
    pass_through! {
        fn write_boolean(value: bool);
        fn write_byte(value: i8);
        fn write_short(value: i16);
        fn write_integer(value: i32);
        fn write_long(value: i64);
        fn write_float(value: f32);
        fn write_double(value: f64);
        fn write_big_integer(value: &BigInteger);
        fn write_big_decimal(value: &BigDecimal);
        fn write_blob(value: &[u8]);
        fn write_timestamp(value: &DateTime);
        fn write_document(value: &Document);
        fn write_null();
    }

    /// The write of a string, which is the one a member with NO VALUE is dropped by.
    fn write_string(
        &mut self,
        schema: &Schema,
        value: &str,
    ) -> ::std::result::Result<(), SerdeError> {
        if self.unsent(schema, Some(value)) {
            return Ok(());
        }
        self.inner.write_string(schema, value)
    }

    /// A nested structure is written under the same rule as the record carrying it.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> ::std::result::Result<(), SerdeError> {
        if self.unsent(schema, None) {
            return Ok(());
        }
        let unsent = self.unsent;
        let when = self.when;
        self.inner.write_struct(
            schema,
            &ValuedMembers {
                record: value,
                unsent,
                when,
            },
        )
    }

    /// A list's elements are written to the serializer this hands the closure, so
    /// the rule reaches the structures inside `subDomains`.
    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> ::std::result::Result<(), SerdeError>,
    ) -> ::std::result::Result<(), SerdeError> {
        if self.unsent(schema, None) {
            return Ok(());
        }
        let unsent = self.unsent;
        let when = self.when;
        self.inner.write_list(schema, &|elements| {
            write_elements(&mut Valued {
                inner: elements,
                unsent,
                when,
            })
        })
    }

    /// A map's entries, for the same reason as a list's elements.
    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> ::std::result::Result<(), SerdeError>,
    ) -> ::std::result::Result<(), SerdeError> {
        if self.unsent(schema, None) {
            return Ok(());
        }
        let unsent = self.unsent;
        let when = self.when;
        self.inner.write_map(schema, &|entries| {
            write_entries(&mut Valued {
                inner: entries,
                unsent,
                when,
            })
        })
    }
}

/// The shape schema of each structure this projection completes, as the member list
/// `Complete::fill_absent` walks. Taking it from the record's own generated
/// [`Schema`] is what keeps this file out of the way of a model change: a member
/// added to `App`, `Branch` or `Backend` appears here on its own.
static APP_SHAPE: &Schema = &App::SCHEMA;
static BRANCH_SHAPE: &Schema = &Branch::SCHEMA;
static BACKEND_SHAPE: &Schema = &Backend::SCHEMA;

/// The shape a STRUCTURE member of a completed record targets, for the members whose
/// target this projection completes too — `Branch$backend`, which Amplify answers as
/// `{"stackArn": null}` rather than `{}` for a branch whose Gen 2 deployment has not
/// provisioned a CloudFormation stack.
///
/// The lookup is keyed by the PARENT shape as well as the member name, so an entry
/// cannot mis-fire on a same-named member of another shape.
///
/// A structure the CALLER configures (`App$autoBranchCreationConfig`,
/// `App$cacheConfig`, `App$jobConfig`, `App$wafConfiguration`,
/// `App$productionBranch`) is NOT completed: it is echoed with the members the
/// configuration carries, which is what `tests/all/app_branch_updates.rs` pins and
/// what every recording of this service shows. A member whose target is not a
/// structure, or a structure inside a list (whose element member is named `member`),
/// answers `None` and is written exactly as the record wrote it.
fn nested_shape(parent: &Schema, member: &Schema) -> Option<&'static Schema> {
    match (parent.shape_id().shape_name(), member.member_name()?) {
        ("Branch", "backend") => Some(BACKEND_SHAPE),
        _ => None,
    }
}

/// `record`, written so that EVERY member of `shape` reaches the wire: the members
/// the record writes itself, the ones named in `unvalued` as JSON `null`, and the
/// ones it wrote nothing for as JSON `null` too.
struct CompleteRecord<'r> {
    record: &'r dyn SerializableStruct,
    shape: &'static Schema,
    unvalued: &'r [&'static str],
}

impl SerializableStruct for CompleteRecord<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let mut completing = Complete {
            inner: serializer,
            shape: self.shape,
            unvalued: self.unvalued,
            written: Vec::new(),
        };
        self.record.serialize_members(&mut completing)?;
        completing.fill_absent()
    }
}

/// A [`ShapeSerializer`] that writes the members in `unvalued` as JSON `null`, notes
/// which members the record wrote, and passes every other write straight through.
///
/// It wraps the serializer the framework hands to `serialize_members`, so the record
/// writes its members exactly as the model says and only the named ones are
/// re-projected. A nested structure this service completes is re-wrapped (see
/// [`nested_shape`]); anything else below this structure is written by the inner
/// serializer untouched.
struct Complete<'s, 'n> {
    inner: &'s mut dyn ShapeSerializer,
    shape: &'static Schema,
    unvalued: &'n [&'static str],
    /// The `member_index` of every member the record wrote — the identity the
    /// generated schema gives a member, so noting one allocates nothing.
    written: Vec<usize>,
}

impl Complete<'_, '_> {
    /// Is `member` one of the members this record has no value for?
    fn has_no_value(&self, member: &Schema) -> bool {
        member
            .member_name()
            .is_some_and(|name| self.unvalued.contains(&name))
    }

    /// Note that the record wrote `member`, whatever it wrote for it.
    fn written_member(&mut self, member: &Schema) {
        if let Some(index) = member.member_index() {
            self.written.push(index);
        }
    }

    /// Write JSON `null` for every member of this record's shape that the record
    /// wrote nothing for.
    fn fill_absent(&mut self) -> ::std::result::Result<(), SerdeError> {
        let shape = self.shape;
        for member in shape.members() {
            let Some(index) = member.member_index() else {
                continue;
            };
            if !self.written.contains(&index) {
                self.inner.write_null(member)?;
            }
        }
        Ok(())
    }
}

/// Emit the [`ShapeSerializer`] methods that write a struct member: `null` for a
/// member with no value, otherwise the write the record asked for — and in both cases
/// a note that the member reached the wire.
///
/// Every member write is routed through the same check — a member is a member
/// whatever its type — so the projection cannot be right for a string and wrong for
/// a list.
macro_rules! null_or_pass_through {
    ($( fn $method:ident($($arg:ident: $ty:ty),*); )*) => {
        $(
            fn $method(
                &mut self,
                schema: &Schema,
                $($arg: $ty),*
            ) -> ::std::result::Result<(), SerdeError> {
                self.written_member(schema);
                if self.has_no_value(schema) {
                    return self.inner.write_null(schema);
                }
                self.inner.$method(schema, $($arg),*)
            }
        )*
    };
}

impl ShapeSerializer for Complete<'_, '_> {
    null_or_pass_through! {
        fn write_list(
            write_elements: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
        fn write_map(
            write_entries: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
        fn write_boolean(value: bool);
        fn write_byte(value: i8);
        fn write_short(value: i16);
        fn write_integer(value: i32);
        fn write_long(value: i64);
        fn write_float(value: f32);
        fn write_double(value: f64);
        fn write_big_integer(value: &BigInteger);
        fn write_big_decimal(value: &BigDecimal);
        fn write_string(value: &str);
        fn write_blob(value: &[u8]);
        fn write_timestamp(value: &DateTime);
        fn write_document(value: &Document);
    }

    /// A nested structure is written as completely as the record carrying it: the
    /// value is re-wrapped in a [`CompleteRecord`] over its own shape, so its unset
    /// members are JSON `null` instead of missing keys.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        if self.has_no_value(schema) {
            return self.inner.write_null(schema);
        }
        match nested_shape(self.shape, schema) {
            Some(shape) => self.inner.write_struct(
                schema,
                &CompleteRecord {
                    record: value,
                    shape,
                    unvalued: &[],
                },
            ),
            None => self.inner.write_struct(schema, value),
        }
    }

    fn write_null(&mut self, schema: &Schema) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        self.inner.write_null(schema)
    }
}
