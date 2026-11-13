// SPDX-License-Identifier: CC0-1.0

//! The DATA events of this service's identity data plane: the five calls CloudTrail
//! delivers as events of the `Data` category over the identity pool they acted on.
//!
//! CloudTrail splits this service's calls in two. The ADMINISTRATION of identity pools —
//! the calls a client makes with AWS developer credentials — is a management event, recorded
//! in the account's event history where `LookupEvents` reports it
//! ([`super::record_served_call`]). The identity DATA PLANE is not: a call that mints or
//! exchanges an identity acts on the CONTENTS of a pool rather than on the account's
//! configuration, so it is a data event — never in event history, and delivered to a trail's
//! bucket only while one of that trail's advanced event selectors admits data events of the
//! identity-pool resource type ([`service_common::data_events`]).
//!
//! What a record of such a call carries is the same seam every recorder in this tree uses:
//! the caller the request was signed with (or the unknown principal, for the unsigned calls
//! the model marks `@optionalAuth`), the pool the call acted on as the event's one resource,
//! and the parameters it carried with every secret withheld — a `Logins` value is
//! `@sensitive` in this model, so the record keys the providers presented and replaces each
//! token with the redaction literal.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use ::service_common::data_events::DataEventDelivery;
use ::service_common::management_events::{
    AuditedResource, ServedApiCall, HIDDEN_SENSITIVE_VALUE,
};
use ::service_common::trail_log_files::DeliveredLogFile;

/// What a handler of this service's data plane reports about the call it has just served,
/// for that call's own data-event record.
pub struct ServedDataCall<'a> {
    /// The operation's modelled name (`Self::NAME`), as the event's `eventName`.
    pub operation: &'a str,
    /// The Region the REQUEST was signed for, which is the Region a call that acts on no
    /// stored pool is recorded in ([`recorded_region`]).
    pub region: &'a str,
    /// Whether the call only REPORTED state, which the record carries as `readOnly` and an
    /// advanced event selector can narrow a trail's data events by.
    pub read_only: bool,
    /// The identity pool the call acted on, when it names or resolves one: the resource the
    /// record names, and what a selector's `resources.type` field selects the call by. A
    /// call that resolves no pool records no resource rather than one built from an
    /// identifier nothing resolves — and is therefore admitted by no selector that names a
    /// resource type.
    pub identity_pool_id: Option<String>,
    /// The parameters the request carried, named as the event document names them and with
    /// every secret withheld ([`recorded_login_values`]).
    pub request_parameters: ::serde_json::Value,
    /// The values the response carried, for a response whose members are not secrets;
    /// `None` for a call that answers nothing, or one whose answer is a credential or a
    /// token.
    pub response_elements: Option<::serde_json::Value>,
}

/// The Region a data event of this service is recorded in: the Region of the identity pool
/// the call acted on, and the Region the REQUEST named for a call that acted on no pool.
///
/// An identity pool is a REGIONAL resource and its own id names the Region it lives in
/// (`IdentityPoolId`: "an identity pool ID in the format REGION:GUID"), so a call on that
/// pool's contents is an event of that Region whatever endpoint the caller reached it
/// through. It has to be read off the pool rather than off the request, because the identity
/// flow is called with NO credential at all — the model marks four of these five operations
/// `@optionalAuth` — and an unsigned request carries no credential scope to read a Region
/// from. Which Region an event belongs to is what decides whether a single-Region trail
/// delivers it.
fn recorded_region(call: &ServedDataCall<'_>) -> String {
    call.identity_pool_id
        .as_deref()
        .map_or_else(|| call.region.to_string(), |pool| super::region_of(pool).to_string())
}

/// Build the data-event record of the call this handler has just served, and hold it until
/// the response that answers it exists ([`service_common::data_events::hold_data_event`]) —
/// which is the only moment the id the caller is answered under, and so the record's
/// `requestID`, is known.
///
/// The LAST thing a data-plane handler does, whatever the call answered: CloudTrail records a
/// failed call too, with the error code and the message its caller was answered with, so an
/// audit pipeline can tell a refused data-plane call from one that never happened. A call the
/// scaffold refused as not implemented is not one this emulator served and is not recorded.
///
/// No secret is written: a provider token, a minted OpenID token and a vended session are
/// the caller's own and are left out of both documents by the handler that fills them.
pub fn hold_data_event<T, E>(
    engine: &(impl Send + Sync + 'static),
    parts: &::http::request::Parts,
    call: &ServedDataCall<'_>,
    outcome: &Result<T, E>,
) where
    E: ::smithy_server_runtime::OperationError + ::std::fmt::Display,
{
    if outcome.as_ref().err().is_some_and(|error| {
        error.shape_name() == ::service_common::audited_operation::NOT_IMPLEMENTED_SHAPE_NAME
            && error.http_status() == ::service_common::audited_operation::NOT_IMPLEMENTED_STATUS
    }) {
        return;
    }
    let (error_code, error_message) =
        ::service_common::management_events::recorded_failure(outcome);
    // Whether this service's PERMISSION CHECK stopped the call, exactly as a management
    // record reads it ([`super::record_served_call`]): a call stopped there was never
    // dispatched, so its record says that a caller asked for something it may not do and not
    // what the refused request held.
    let stopped_at_authorization = outcome.as_ref().err().is_some_and(|error| {
        error.shape_name() == crate::scaffolded::ACCESS_DENIED_CODE
            && error.http_status() == crate::scaffolded::ACCESS_DENIED_STATUS
    });
    let caller = ::service_common::management_events::recorded_caller(parts);
    let session = super::recorded_session(engine, caller.access_key_id.as_deref());
    ::service_common::data_events::hold_data_event(
        ServedApiCall {
            event_name: call.operation.to_string(),
            event_source: super::EVENT_SOURCE.to_string(),
            aws_region: recorded_region(call),
            read_only: call.read_only,
            access_key_id: caller.access_key_id,
            user_name: session
                .as_ref()
                .map_or(caller.user_name, |session| session.user_name.clone()),
            principal_type: session.as_ref().map_or_else(
                || caller.principal_type.to_string(),
                |session| session.principal_type.clone(),
            ),
            // The unique id of the IAM entity a LONG-TERM credential speaks for is IAM's to
            // mint and this service resolves none; a session's own principal id is the
            // session's, and the account's session directory holds it.
            principal_id: session.as_ref().map(|session| session.principal_id.clone()),
            // Every data-plane call is made either with an AWS credential or with none at
            // all: the end user a `Logins` token speaks for is not the PRINCIPAL of the
            // call, which is what `identityProvider` would name.
            identity_provider: None,
            session,
            account_id: ::engine::emulator_account_id().to_string(),
            user_agent: ::service_common::management_events::recorded_user_agent(parts),
            resources: call
                .identity_pool_id
                .as_deref()
                .map(|identity_pool_id| {
                    AuditedResource::new(
                        super::IDENTITY_POOL_RESOURCE_TYPE_NAME,
                        super::identity_pool_arn(identity_pool_id),
                    )
                })
                .into_iter()
                .collect(),
            request_parameters: ::service_common::management_events::recorded_request_parameters(
                call.request_parameters.clone(),
                stopped_at_authorization,
            ),
            response_elements: call.response_elements.clone(),
            error_code,
            error_message,
        },
        SystemTime::now(),
    );
}

/// The `Logins` a request presented, as its record reports them: every provider NAME the
/// request carried, each keyed to the redaction literal in place of the token it presented.
///
/// The model marks the VALUE secret — `LoginsMap`'s value targets `IdentityProviderToken`,
/// which is `@sensitive` (`smithy_query(query="shape",shape="LoginsMap")`) — and CloudTrail
/// reports a withheld value as [`HIDDEN_SENSITIVE_VALUE`], so a reader sees WHICH provider
/// authenticated the call without the token ever being stored. `None` for a request that
/// presented no login at all.
#[must_use]
pub fn recorded_login_values(logins: Option<&HashMap<String, String>>) -> Option<::serde_json::Value> {
    let logins = logins?;
    if logins.is_empty() {
        return None;
    }
    let mut recorded = ::serde_json::Map::new();
    // Sorted, so one call's record reads the same whichever order the map happened to hold.
    let mut providers: Vec<&String> = logins.keys().collect();
    providers.sort();
    for provider in providers {
        recorded.insert(
            provider.clone(),
            ::serde_json::Value::String(HIDDEN_SENSITIVE_VALUE.to_string()),
        );
    }
    Some(recorded.into())
}

/// The credential a served vend answered, as its record reports it: the session's access key
/// id, the session token it is presented with, and when it expires.
///
/// What is WITHHELD is the SECRET KEY, and only it: CloudTrail records the credential a vend
/// answered so a reader can tell which session acted later in the trail, and the secret key
/// is the one member no record carries — the same half the record of the assume behind the
/// vend carries ([`super::record_vended_assume`]). Nothing here is read back by this
/// emulator: the record is what the account's trail delivers, not a store of credentials.
///
/// `None` for a call that answered no credential at all.
#[must_use]
pub fn recorded_credentials(vended: Option<VendedCredential<'_>>) -> Option<::serde_json::Value> {
    let vended = vended?;
    let expiration = vended
        .expiration
        .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok());
    Some(::service_common::management_events::recorded_members([
        ("accessKeyId", ::serde_json::json!(vended.access_key_id)),
        ("sessionToken", ::serde_json::json!(vended.session_token)),
        ("expiration", ::serde_json::json!(expiration)),
    ]))
}

/// The members of a vended credential a record reports, as the handler reads them off the
/// response it answered.
pub struct VendedCredential<'a> {
    /// The session's access key id, which names the credential.
    pub access_key_id: Option<&'a str>,
    /// The session token the credential is presented with.
    pub session_token: Option<&'a str>,
    /// When the credential stops being valid.
    pub expiration: Option<&'a ::aws_smithy_types::DateTime>,
}

/// What this service contributes to the delivery of one of its data events: the log files
/// the account's trails deliver it as.
///
/// Holds the shared engine — the trail projection the delivery is decided by lives in the
/// log that engine carries — so it is generic over the engine type exactly as the service is.
pub struct IdentityPoolDataEvents<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> IdentityPoolDataEvents<Eng> {
    /// Build the deliverer from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> DataEventDelivery for IdentityPoolDataEvents<Eng> {
    fn delivered(
        &self,
        call: ServedApiCall,
        served_at: SystemTime,
        answered_as: &str,
    ) -> Vec<DeliveredLogFile> {
        let Some(log) = super::management_event_log(&*self.engine) else {
            // Not silent: an undelivered data event leaves no trace in the account's trail
            // by definition, so it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the data-plane call was served \
                 with no trail able to deliver it"
            );
            return Vec::new();
        };
        // The guard is BOUND rather than used as a temporary: a local is released before the
        // handle it borrows, which a temporary at the end of this block would not be.
        let delivering = log.lock();
        delivering.data_event_log_files(call, served_at, answered_as)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One call of the data plane, as a handler reports it.
    fn call<'a>(region: &'a str, identity_pool_id: Option<&str>) -> ServedDataCall<'a> {
        ServedDataCall {
            operation: "GetId",
            region,
            read_only: false,
            identity_pool_id: identity_pool_id.map(ToString::to_string),
            request_parameters: ::serde_json::json!({}),
            response_elements: None,
        }
    }

    /// RULE: a data event belongs to the Region of the POOL it acted on, and to the Region
    /// the request named only when it acted on no pool.
    #[test]
    fn a_data_event_is_recorded_in_the_region_of_the_pool_it_acted_on() {
        // The rows differ along the Region the request named, along whether a pool was
        // resolved, and along the Region that pool's own id names.
        let rows = [
            (
                "an unsigned call on a pool of another Region",
                super::super::DEFAULT_REGION,
                Some("us-west-2:00000000-0000-0000-0000-000000000000"),
                "us-west-2",
            ),
            (
                "a signed call on a pool of the Region it was signed for",
                "eu-central-1",
                Some("eu-central-1:00000000-0000-0000-0000-000000000000"),
                "eu-central-1",
            ),
            (
                "a signed call on a pool of another Region",
                "eu-central-1",
                Some("ap-southeast-2:00000000-0000-0000-0000-000000000000"),
                "ap-southeast-2",
            ),
            (
                "a call that resolved no pool",
                "eu-central-1",
                None,
                "eu-central-1",
            ),
            (
                "a call naming an identifier that spells no Region",
                "eu-central-1",
                Some("not-a-pool-id"),
                super::super::DEFAULT_REGION,
            ),
        ];
        for (case, request_region, identity_pool_id, expected) in rows {
            let actual = recorded_region(&call(request_region, identity_pool_id));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the event in {expected}, got {actual}"
            );
        }
    }

    /// RULE: a record keys the providers a request presented and carries the redaction
    /// literal in place of every token.
    #[test]
    fn a_recorded_login_keys_its_provider_and_withholds_its_token() {
        /// One row: the case, the `Logins` entries the request presented, and the document
        /// its record carries for them.
        type Row = (
            &'static str,
            Vec<(&'static str, &'static str)>,
            ::serde_json::Value,
        );
        let rows: Vec<Row> = vec![
            (
                "one presented provider",
                vec![("graph.facebook.com", "a.live.token")],
                ::serde_json::json!({"graph.facebook.com": HIDDEN_SENSITIVE_VALUE}),
            ),
            (
                "two presented providers",
                vec![
                    ("accounts.google.com", "another.live.token"),
                    ("graph.facebook.com", "a.live.token"),
                ],
                ::serde_json::json!({
                    "accounts.google.com": HIDDEN_SENSITIVE_VALUE,
                    "graph.facebook.com": HIDDEN_SENSITIVE_VALUE,
                }),
            ),
            ("no login at all", Vec::new(), ::serde_json::Value::Null),
        ];
        for (case, presented, expected) in rows {
            let logins: HashMap<String, String> = presented
                .iter()
                .map(|(provider, token)| ((*provider).to_string(), (*token).to_string()))
                .collect();
            let actual = recorded_login_values(Some(&logins))
                .unwrap_or(::serde_json::Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
            for (_, token) in &presented {
                assert!(
                    !actual.to_string().contains(token),
                    "case {case:?}: no record may carry the token {token:?}, got {actual}"
                );
            }
        }
        assert_eq!(
            recorded_login_values(None),
            None,
            "case \"a request carrying no Logins member\": expected no logins at all"
        );
    }

    /// RULE: the record of a vend names the credential it answered — its access key id, its
    /// session token and its expiry — and the SECRET KEY reaches it never.
    #[test]
    fn a_recorded_vend_names_the_credential_but_never_its_secret_key() {
        let expires_at = ::aws_smithy_types::DateTime::from_secs(1_793_000_000);
        // The rows differ along which members the vended credential carried.
        let rows: Vec<(&str, Option<VendedCredential<'_>>, ::serde_json::Value)> = vec![
            (
                "a whole vended session",
                Some(VendedCredential {
                    access_key_id: Some("ASIAEXAMPLEVENDED001"),
                    session_token: Some("a.session.token"),
                    expiration: Some(&expires_at),
                }),
                ::serde_json::json!({
                    "accessKeyId": "ASIAEXAMPLEVENDED001",
                    "sessionToken": "a.session.token",
                    "expiration": "2026-10-26T07:33:20Z",
                }),
            ),
            (
                "a credential carrying no expiry",
                Some(VendedCredential {
                    access_key_id: Some("ASIAEXAMPLEVENDED002"),
                    session_token: Some("another.session.token"),
                    expiration: None,
                }),
                ::serde_json::json!({
                    "accessKeyId": "ASIAEXAMPLEVENDED002",
                    "sessionToken": "another.session.token",
                }),
            ),
            ("a call that answered none", None, ::serde_json::Value::Null),
        ];
        for (case, vended, expected) in rows {
            let actual = recorded_credentials(vended).unwrap_or(::serde_json::Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
            assert!(
                !actual.to_string().contains("secret"),
                "case {case:?}: no record may name a secret key, got {actual}"
            );
        }
    }
}
