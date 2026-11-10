// SPDX-License-Identifier: CC0-1.0

//! The one half of a constraint refusal the framework's walk cannot state — the
//! COLLECTION a clause about a collection member is about — served as the per-operation
//! wrapper that is the only layer able to state it.
//!
//! # The rule
//!
//! A value outside a constraint the model declares is refused before any handler runs,
//! over one clause per violation, each naming the value that was rejected, the member path
//! it sat at and the requirement it failed. The value is the only place the request's own
//! input is reported back, so a client comparing it against what it sent has to see its
//! own input.
//!
//! A `@length` on a map or a list is checked ON THE MEMBER, before the walk descends into
//! the entries, so the only rendering the framework has for the rejected value is a
//! SUMMARY of its size — `<n> items` for a list, `<n> entries` for a map — which is not a
//! value any caller sent (`vendor/smithy-schema-server/src/constraints.rs`). AWS states
//! the collection itself there: a map as `{<key>=<value>, …}`, a list as `[<element>, …]`.
//!
//! # Why a wrapper
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is
//! handed the violations alone and never the request they were found in, so that value
//! cannot be recovered there. [`QuotingSubmittedCollections`] is where it is: a
//! per-operation wrapper, which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the violations and
//! the submitted collections come from this crate's shared collecting walk
//! ([`crate::validation::refusing_violations_with_mapped_echoes`], which runs the
//! protocol's own deserializer and the framework's own `constraints::validate` and owns
//! every bound on what one clause may quote), the restoring rule is
//! [`crate::validation_message::quoting_the_submitted_collection`], each clause is the
//! framework's own `Violation::message`, the envelope around them is the framework's own,
//! and the status and the wire envelope come from the framework's refusal channel. A
//! refusal with NO such clause is left to that channel untouched.
//!
//! # What a service states, and what it does not
//!
//! THE HOME for this wrapper: a service states only the facts that are its own — the error
//! CODE its refusals carry, the ORDER it states a map's entries in, how it answers a
//! rendered response, and which of its operations are served through the pass
//! ([`StatedClauseTable`]). Everything else is here, so two services cannot disagree about
//! how one collection is quoted.
//!
//! A map's entries have an order on the wire, none in the value the codec builds from it (a
//! string map arrives as a `HashMap`), and possibly a third in the clause the service
//! states — so the shared walk, which reads the BUILT input, can only state one map in key
//! order. The order the request wrote is still in the request, which this layer has, so a
//! service that states another order supplies it as a [`MapEntryOrder`] over the entries
//! read back from the request ([`submitted_map_entries`]); [`in_submitted_order`] is the
//! order the request carried, which is what a service whose recordings show the submitted
//! order passes.
//!
//! What this layer may NOT do is enlarge the clause: every bound on what one message quotes
//! belongs to the shared walk, so a restated map is taken only when it renders to exactly
//! the length that walk had already decided to carry — the same entries in another order,
//! and therefore the same bytes. A clause that fails any of the guards keeps the shared
//! walk's own rendering: a value that is not a map (a list, whose order the built value does
//! keep), a member below the top level of the body, a body this cannot read, a member whose
//! entry count differs from the one the walk counted, and an order the service's own rule
//! declines to state.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refused,
    Service, Violation,
};

use crate::validation::{refusing_violations_with_mapped_echoes, rendered_map};
use crate::validation_message::{
    envelope_over, map_entry_count, quoting_the_submitted_collection,
};

/// The order a service states a map's entries in inside a clause, over the entries as the
/// REQUEST carried them — or `None` where that service's own rule declines to state an
/// order for them, which keeps the shared walk's own rendering.
pub type MapEntryOrder = fn(&[(String, String)]) -> Option<Vec<(&str, &str)>>;

/// The entries in the order the request submitted them: the order the caller's own JSON
/// object wrote them.
///
/// The [`MapEntryOrder`] of a service whose recordings quote a map in the order it was
/// sent, which is the order the validating layer in front of such a service iterates a
/// map that KEEPS its insertion order.
#[must_use]
pub fn in_submitted_order(entries: &[(String, String)]) -> Option<Vec<(&str, &str)>> {
    Some(
        entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect(),
    )
}

/// How a service answers a response one of its layers rendered — the seam a service whose
/// wire form differs from the one a renderer wrote states that difference at (the member an
/// error body carries its text in, say).
pub type AnsweredAs = fn(RtResponse) -> RtResponse;

/// A response answered exactly as its renderer wrote it: the [`AnsweredAs`] of a service
/// that restates nothing.
#[must_use]
pub fn as_rendered(resp: RtResponse) -> RtResponse {
    resp
}

/// The facts ONE service states about its own constraint clauses: the error code they
/// carry, the order it states a map's entries in, and how it answers a rendered response.
#[derive(Clone, Copy)]
pub struct StatedClause {
    /// The error code a constraint refusal of this service carries — the one its own
    /// refusal seam picks for the class, because the restated message has to travel with
    /// the code it would have travelled with: a client branches on the code.
    pub refusal_code: &'static str,
    /// The order this service states a map's entries in ([`MapEntryOrder`]).
    pub entry_order: MapEntryOrder,
    /// How this service answers a rendered response ([`AnsweredAs`]).
    pub answered_as: AnsweredAs,
}

impl StatedClause {
    /// The clause of a service that quotes a map in the order the request submitted it and
    /// answers a rendered response unchanged, under `refusal_code`.
    #[must_use]
    pub fn under(refusal_code: &'static str) -> Self {
        Self {
            refusal_code,
            entry_order: in_submitted_order,
            answered_as: as_rendered,
        }
    }

    /// The same clause, with the order this service states a map's entries in.
    #[must_use]
    pub fn ordering_entries(self, entry_order: MapEntryOrder) -> Self {
        Self {
            entry_order,
            ..self
        }
    }

    /// The same clause, with how this service answers a rendered response.
    #[must_use]
    pub fn answered(self, answered_as: AnsweredAs) -> Self {
        Self {
            answered_as,
            ..self
        }
    }
}

/// One modelled operation, served so that a constraint clause about a collection member
/// states the collection the request carried.
///
/// It makes no routing decision and holds no state of its own: the operation it wraps is
/// the one the generated routing table names for the same key, built from the SAME shared
/// engine, and a request this wrapper does not refuse is handed to it untouched — so the
/// framework still owns the whole deserialize -> validate -> handle -> serialize sequence
/// and every status, shape and message it answers with.
///
/// The cost is DELIBERATE: a request's body is read twice, here and in the operation's own
/// sequence, because the framework hands no already-built input to an operation. Which
/// operations pay it is the service's own choice ([`StatedClauseTable::quoting`] against
/// [`StatedClauseTable::answering`]).
pub struct QuotingSubmittedCollections<Op> {
    operation: Op,
    stated: StatedClause,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, stated: StatedClause) -> Self {
        Self { operation, stated }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is
            // rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across that
            // await would make this future non-`Send`.
            if let Some(refused) = quoting_refusal::<Op>(svc, req, &self.stated) {
                return (self.stated.answered_as)(refused);
            }
            (self.stated.answered_as)(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One modelled operation, served so that its responses are answered as this service
/// answers them ([`AnsweredAs`]) and NOTHING else changes.
///
/// For an operation whose model declares no bounded collection member, where the pass
/// above could never produce a clause and would only read the body a second time.
pub struct AnsweringAs<Op> {
    operation: Op,
    answered_as: AnsweredAs,
}

impl<Op> AnsweringAs<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, answered_as: AnsweredAs) -> Self {
        Self {
            operation,
            answered_as,
        }
    }
}

impl<Op> ErasedOperation for AnsweringAs<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { (self.answered_as)(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes, for the same reason as above.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The routing table a service serves its operations through so that their error bodies
/// are the ones IT answers with: every operation wrapped, keyed by the routing key the
/// protocol resolves (the operation's own modelled name).
///
/// A service builds one in its `EmulatedService::new` and answers
/// `Service::override_operation` from it; [`quoting`](Self::quoting) and
/// [`answering`](Self::answering) each take one already-constructed operation and file it
/// under [`Operation::NAME`], so a wrapped operation cannot land under a key the protocol
/// never resolves.
pub struct StatedClauseTable {
    table: OperationRoutingTable,
    stated: StatedClause,
}

impl StatedClauseTable {
    /// An empty table whose operations state `stated`'s clauses.
    #[must_use]
    pub fn new(stated: StatedClause) -> Self {
        Self {
            table: OperationRoutingTable::new(),
            stated,
        }
    }

    /// Serve `operation` with a constraint clause about a collection member QUOTING the
    /// collection the request submitted — for an operation whose model declares a bounded
    /// collection member, which is the only kind that can produce such a clause.
    pub fn quoting<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(QuotingSubmittedCollections::new(operation, self.stated)),
        );
        self
    }

    /// Serve `operation` with its responses answered as this service answers them and no
    /// collection pass at all.
    pub fn answering<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(AnsweringAs::new(operation, self.stated.answered_as)),
        );
        self
    }

    /// The table, for the service to hold and route from.
    #[must_use]
    pub fn finish(self) -> OperationRoutingTable {
        self.table
    }
}

/// The refusal `req` earns from `Op`'s own constraints when at least one of its clauses is
/// about a COLLECTION whose submitted value could be stated, or `None` — which is every
/// other request, including one refused only by clauses this changes nothing about, so the
/// framework's own channel answers it exactly as before.
pub fn quoting_refusal<Op: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    stated: &StatedClause,
) -> Option<RtResponse> {
    let refusing = refusing_violations_with_mapped_echoes::<Op>(svc.definition().protocol(), req).0;
    let mut quoted_a_collection = false;
    let violations: Vec<Violation> = refusing
        .into_iter()
        .map(|(violation, echo)| {
            let quoted = quoting_the_submitted_collection(violation.clone(), echo);
            let stated = as_stated(&violation, quoted, req, stated.entry_order);
            if stated.value == violation.value {
                return violation;
            }
            quoted_a_collection = true;
            stated
        })
        .collect();
    if !quoted_a_collection {
        return None;
    }
    // Every clause is the framework's own sentence over the violation as stated, and the
    // envelope over them is the framework's own too, so the ONE difference from the refusal
    // that channel would have rendered is the collection a clause about a collection
    // quotes.
    let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
    let message = envelope_over(&clauses)?;
    Some(svc.definition().refusal_response(
        ::aws_smithy_schema_server::ProtocolError::Invalid(violations),
        Some(Refused {
            code: stated.refusal_code.to_string(),
            message,
        }),
        req,
    ))
}

/// `quoted` — the shared rendering of the collection `violation` is about — with a MAP's
/// entries restated in the order the service states them (`order`) rather than in the key
/// order the built input leaves them in.
///
/// Only the ORDER changes: the entries, the braces and the `<key>=<value>` form are the
/// shared rendering's ([`rendered_map`]). See the module header for every guard that keeps
/// `quoted` instead.
fn as_stated(
    violation: &Violation,
    quoted: Violation,
    req: &RtRequest,
    order: MapEntryOrder,
) -> Violation {
    let (Some(summary), Some(rendered)) = (violation.value.as_deref(), quoted.value.as_deref())
    else {
        return quoted;
    };
    let Some(entries) = map_entry_count(summary) else {
        return quoted;
    };
    if violation.path.contains('.') {
        return quoted;
    }
    let Some(submitted) = submitted_map_entries(req.body().bytes().unwrap_or_default(), &violation.path)
    else {
        return quoted;
    };
    if submitted.len() != entries {
        return quoted;
    }
    let Some(ordered) = order(&submitted) else {
        return quoted;
    };
    let restated = rendered_map(&ordered);
    if restated.len() != rendered.len() {
        return quoted;
    }
    Violation {
        value: Some(restated),
        ..quoted
    }
}

/// The entries the request's `member` carried, in the order its JSON object wrote them, or
/// `None` when the body is not an object, carries no such member, or carries one that is
/// not a map of strings.
///
/// The JSON is read by `serde_json`'s own parser, through `serde`'s deserializer seam:
/// nothing here reads the body's bytes itself. The seam is what keeps the ORDER, which a
/// parse into a `Value` would not — that type holds its members in a map of its own.
///
/// It reads the body the server has already buffered and the framework has already
/// deserialized into the operation's input, over the one member a clause is about and
/// skipping every other without building it, so it adds no growth a request can drive
/// beyond that one parse; and what it produces is only used while it renders to the length
/// the shared walk's own bounds had already admitted (see [`as_stated`]).
///
/// The member is matched without regard to case because a violation names it in the
/// lowerCamel path the framework states a clause at, while the body carries the wire name
/// the model declares. A model that declared two members of one shape differing only in
/// case would make the first one the body carries the one read.
#[must_use]
pub fn submitted_map_entries(body: &[u8], member: &str) -> Option<Vec<(String, String)>> {
    let mut deserializer = ::serde_json::Deserializer::from_slice(body);
    ::serde::de::DeserializeSeed::deserialize(MemberEntries { member }, &mut deserializer)
        .ok()
        .flatten()
}

/// Reads ONE named member of a JSON object as the string map it holds, keeping the order
/// the document wrote the entries in. Every other member is skipped without being built.
struct MemberEntries<'a> {
    member: &'a str,
}

impl<'de> ::serde::de::DeserializeSeed<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn deserialize<D: ::serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> ::serde::de::Visitor<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(formatter, "a JSON object carrying the member {}", self.member)
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut members: A,
    ) -> Result<Self::Value, A::Error> {
        let mut found: Option<Vec<(String, String)>> = None;
        while let Some(name) = members.next_key::<String>()? {
            if found.is_none() && name.eq_ignore_ascii_case(self.member) {
                found = Some(members.next_value::<OrderedStringMap>()?.0);
            } else {
                members.next_value::<::serde::de::IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// A JSON object of strings as the entries it holds, in document order.
struct OrderedStringMap(Vec<(String, String)>);

impl<'de> ::serde::Deserialize<'de> for OrderedStringMap {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> ::serde::de::Visitor<'de> for Entries {
            type Value = OrderedStringMap;

            fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str("a JSON object of strings")
            }

            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut entries: A,
            ) -> Result<Self::Value, A::Error> {
                let mut held: Vec<(String, String)> = Vec::new();
                while let Some(entry) = entries.next_entry::<String, String>()? {
                    held.push(entry);
                }
                Ok(OrderedStringMap(held))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the member a clause is about is read back in the order the document wrote it,
    /// whatever else the body carries — and nothing is read where there is nothing to read.
    #[test]
    fn the_named_member_is_read_back_in_document_order() {
        let body = br#"{"Before":"x","Attributes":{"z":"1","a":"2","m":"3"},"After":[1,2]}"#;
        let entries = submitted_map_entries(body, "attributes").expect("the member is a map");
        assert_eq!(
            entries,
            vec![
                ("z".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
                ("m".to_string(), "3".to_string()),
            ],
            "expected the entries in document order, got {entries:?}"
        );

        // (case, body, member)
        for (case, body, member) in [
            ("no such member", &br#"{"Other":{"a":"1"}}"#[..], "attributes"),
            ("not an object", &br#"[1,2,3]"#[..], "attributes"),
            ("not JSON at all", &b"<Error/>"[..], "attributes"),
            (
                "a member that is not a map of strings",
                &br#"{"Attributes":{"a":1}}"#[..],
                "attributes",
            ),
        ] {
            assert!(
                submitted_map_entries(body, member).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// RULE: the submitted order is the entries as they were read, unchanged — the order a
    /// service whose layer keeps a map's insertion order states.
    #[test]
    fn the_submitted_order_states_the_entries_as_they_were_sent() {
        let entries: Vec<(String, String)> = [("z", "1"), ("a", "2")]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            in_submitted_order(&entries),
            Some(vec![("z", "1"), ("a", "2")]),
            "expected the submitted order"
        );
    }
}
