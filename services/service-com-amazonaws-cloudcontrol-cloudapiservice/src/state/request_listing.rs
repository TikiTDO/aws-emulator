// SPDX-License-Identifier: CC0-1.0

//! Which resource operation requests a listing reports, in what order, and where the next
//! page of them resumes.
//!
//! `ListResourceRequests` "returns existing resource operation requests. This includes
//! requests of all status types"
//! (`smithy_query(query="doc",shape="ListResourceRequests")`), narrowed by the filter
//! criteria the call carries: the operation types and the operation statuses to include.
//! So the requests a listing reports are the records
//! [`crate::state::ComAmazonawsCloudcontrolCloudapiserviceState`] already holds — the same
//! records `GetResourceRequestStatus` resolves one token to — and nothing here mints,
//! moves or forgets one.
//!
//! A request is reported NEWEST FIRST, by the instant it was initiated
//! (`ProgressEvent$EventTime`, "when the resource operation request was initiated"), so a
//! client polling the list sees the request it just started at the front. Two requests
//! initiated at the same instant are ordered by their `RequestToken`, which makes the
//! order a TOTAL one: that is what lets a page resume after the request it stopped at
//! without reporting one twice or skipping one.

use ::std::cmp::Ordering;
use ::std::time::UNIX_EPOCH;

use super::{
    ComAmazonawsCloudcontrolCloudapiserviceState, PageTokenKey, RequestRecord, CURSOR_SEPARATOR,
};

/// The scope this listing's page cursors are minted under, so a `NextToken` minted for a
/// resource listing resumes nothing here and one minted here resumes no resource listing.
///
/// A `TypeName` — the scope a resource listing's cursor carries — always holds `::` (its
/// pattern is `^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$`), so this value
/// can never be one.
const REQUEST_LISTING_SCOPE: &str = "ResourceRequests";

/// How a listing words a `NextToken` it will not read.
///
/// A token is opaque, so there is nothing to say about it beyond which member it was: the
/// service states a member it cannot take as `Invalid <Member>`, spelling the member as
/// the model does.
pub const INVALID_NEXT_TOKEN: &str = "Invalid NextToken";

/// One request's place in the listing order: when it was initiated, and its token to
/// settle a tie.
///
/// Ordered NEWEST FIRST, which is the order a page reports and the order a cursor resumes
/// in; the token tie-break makes it total, so two requests initiated in the same instant
/// still have one position each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPosition {
    /// The request's `EventTime` as nanoseconds since the Unix epoch — the instant
    /// itself, carried in a form a cursor can be written in and read back from.
    event_nanos: u128,
    /// The request's `RequestToken`, which distinguishes two requests initiated at the
    /// same instant.
    request_token: String,
}

impl RequestPosition {
    /// Where `record` sits in the listing order.
    pub fn of(record: &RequestRecord) -> Self {
        Self {
            event_nanos: record
                .event_time
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
            request_token: record.request_token.clone(),
        }
    }

    /// This position as the text a page cursor carries.
    fn cursor(&self) -> String {
        format!(
            "{}{CURSOR_SEPARATOR}{}",
            self.event_nanos, self.request_token
        )
    }

    /// The position that text names, or `None` for anything that is not one.
    fn from_cursor(cursor: &str) -> Option<Self> {
        let (event_nanos, request_token) = cursor.split_once(CURSOR_SEPARATOR)?;
        // A `RequestToken` is at least one character long, so a cursor naming none names
        // no request this store could have reported.
        if request_token.is_empty() {
            return None;
        }
        Some(Self {
            event_nanos: event_nanos.parse().ok()?,
            request_token: request_token.to_string(),
        })
    }
}

impl Ord for RequestPosition {
    /// Newest first: the later `EventTime` sorts before the earlier one, and two requests
    /// initiated at the same instant are ordered by their token.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .event_nanos
            .cmp(&self.event_nanos)
            .then_with(|| self.request_token.cmp(&other.request_token))
    }
}

impl PartialOrd for RequestPosition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// "The filter criteria to apply to the requests returned" — the two criteria
/// `ResourceRequestStatusFilter` carries, as the store reads them.
///
/// A criterion that names nothing narrows nothing, so a listing carrying no filter reports
/// every request this store holds.
#[derive(Debug, Default, Clone, Copy)]
pub struct RequestFilter<'a> {
    /// "The operation types to include in the filter" (`Operation`: `CREATE`, `UPDATE`,
    /// `DELETE`).
    pub operations: &'a [String],
    /// "The operation statuses to include in the filter" (`OperationStatus`).
    pub operation_statuses: &'a [String],
}

impl RequestFilter<'_> {
    /// Whether this filter's criteria admit `record`: every criterion that names anything
    /// has to name the record's own value, so two criteria narrow a listing together
    /// rather than widening it.
    fn admits(&self, record: &RequestRecord) -> bool {
        admitted(self.operations, &record.operation)
            && admitted(self.operation_statuses, &record.operation_status)
    }
}

/// Whether one criterion admits `value` — which a criterion naming nothing always does.
fn admitted(criterion: &[String], value: &str) -> bool {
    criterion.is_empty() || criterion.iter().any(|named| named == value)
}

impl ComAmazonawsCloudcontrolCloudapiserviceState {
    /// One window of the resource operation requests this store holds: those `filter`
    /// admits, newest first, from the position after `after`, and at most `limit` of them.
    ///
    /// Reporting only, and the window is applied here rather than at the caller, so a
    /// listing's work is the page it answers rather than the size of the store behind it.
    pub fn list_requests(
        &self,
        filter: &RequestFilter<'_>,
        after: Option<&RequestPosition>,
        limit: usize,
    ) -> Vec<&RequestRecord> {
        let mut window: Vec<(RequestPosition, &RequestRecord)> = self
            .requests
            .values()
            .filter(|record| filter.admits(record))
            .map(|record| (RequestPosition::of(record), record))
            .filter(|(position, _)| after.is_none_or(|after| position > after))
            .collect();
        window.sort_by(|(left, _), (right, _)| left.cmp(right));
        window.truncate(limit);
        window.into_iter().map(|(_, record)| record).collect()
    }
}

/// The `NextToken` that resumes this listing after `position` — opaque to the caller, and
/// authenticated so only a token this store minted for THIS listing resumes it.
pub fn request_page_token(key: &PageTokenKey, position: &RequestPosition) -> String {
    super::encode_page_token(
        key,
        &super::page_cursor(REQUEST_LISTING_SCOPE, &position.cursor()),
    )
}

/// The position a `NextToken` resumes this listing after, or `None` for any token this
/// store did not mint for it — one that is not hex, one whose authenticator does not match
/// the cursor it travels with, one minted for a resource listing, and one that carries no
/// position at all.
pub fn decode_request_page_token(key: &PageTokenKey, token: &str) -> Option<RequestPosition> {
    RequestPosition::from_cursor(&super::decode_page_token(key, REQUEST_LISTING_SCOPE, token)?)
}

#[cfg(test)]
mod tests {
    use ::std::time::{Duration, SystemTime};

    use super::super::{page_cursor, ResourceEffect, OPERATION_CREATE, STATUS_IN_PROGRESS};
    use super::*;

    /// One accepted request record, initiated at `event_time`.
    fn record(request_token: &str, event_time: SystemTime) -> RequestRecord {
        RequestRecord::accepted(
            request_token.to_string(),
            "AWS::Kinesis::Stream".to_string(),
            Some("stream".to_string()),
            OPERATION_CREATE,
            event_time,
            ResourceEffect::Create {
                properties: ::serde_json::json!({ "StreamName": "stream" }),
                identifier_property: "StreamName".to_string(),
                type_version_id: None,
                region: "us-east-1".to_string(),
            },
        )
    }

    /// The listing order is newest first, and total: two requests initiated in the same
    /// instant are still ordered, by their token.
    #[test]
    fn a_position_orders_the_newest_request_first() {
        let instant = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let older = RequestPosition::of(&record("bbbb", instant));
        let newer = RequestPosition::of(&record("aaaa", instant + Duration::from_millis(1)));
        assert!(
            newer < older,
            "case newest-first: expected {newer:?} to sort before {older:?}"
        );
        let same_instant = RequestPosition::of(&record("cccc", instant));
        assert!(
            older < same_instant,
            "case tie-break: expected the lower token {older:?} to sort before \
             {same_instant:?}"
        );
    }

    /// A page cursor round-trips under the key that minted it, and under nothing else —
    /// including the cursor of the resource listing, which shares the key.
    #[test]
    fn a_page_cursor_round_trips_only_for_the_listing_it_was_minted_for() {
        let key = PageTokenKey::new();
        let position = RequestPosition::of(&record(
            "abcd",
            SystemTime::UNIX_EPOCH + Duration::from_nanos(42),
        ));
        let token = request_page_token(&key, &position);
        assert_eq!(
            decode_request_page_token(&key, &token),
            Some(position),
            "a cursor must round-trip for the listing that minted it"
        );
        assert_eq!(
            decode_request_page_token(&PageTokenKey::new(), &token),
            None,
            "a token minted under another key must not decode"
        );
        let resource_token =
            super::super::encode_page_token(&key, &page_cursor("AWS::S3::Bucket", "demo"));
        assert_eq!(
            decode_request_page_token(&key, &resource_token),
            None,
            "a token minted for a resource listing must not resume this one"
        );
        assert_eq!(
            decode_request_page_token(&key, "not-hex"),
            None,
            "a value a caller wrote must not decode"
        );
    }

    /// A criterion that names nothing narrows nothing; criteria that name something narrow
    /// together.
    #[test]
    fn a_filter_admits_a_request_every_criterion_names() {
        let record = record("abcd", SystemTime::now());
        let rows: [(&str, Vec<String>, Vec<String>, bool); 5] = [
            ("no criterion at all", vec![], vec![], true),
            (
                "the request's own operation",
                vec![OPERATION_CREATE.to_string()],
                vec![],
                true,
            ),
            (
                "another operation",
                vec![super::super::OPERATION_DELETE.to_string()],
                vec![],
                false,
            ),
            (
                "the request's operation and status",
                vec![OPERATION_CREATE.to_string()],
                vec![STATUS_IN_PROGRESS.to_string()],
                true,
            ),
            (
                "its operation but another status",
                vec![OPERATION_CREATE.to_string()],
                vec![super::super::STATUS_SUCCESS.to_string()],
                false,
            ),
        ];
        for (case, operations, operation_statuses, expected) in rows {
            let filter = RequestFilter {
                operations: &operations,
                operation_statuses: &operation_statuses,
            };
            assert_eq!(
                filter.admits(&record),
                expected,
                "case {case:?}: expected the filter to admit the request: {expected}"
            );
        }
    }
}
