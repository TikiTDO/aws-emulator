// SPDX-License-Identifier: CC0-1.0

//! The AgentCore *browser* tools plane: the sessions a browser holds.
//!
//! A browser is created through the CONTROL plane, a different service, so a browser is
//! registered here by the first `StartBrowserSession` that names it — the same way a
//! code interpreter is registered by the first session started on it. The record a start
//! writes ([`BrowserSessionRecord`]) is exactly what `GetBrowserSession` reports back,
//! and its status is read off the clock against the time-to-live the request asked for
//! ([`BrowserSessionRecord::status_as_of`]), so no call is needed to make a termination
//! that is already due visible.
//!
//! `StartBrowserSession` is this plane's only growth site and a caller chooses both the
//! `browserIdentifier` and the `clientToken` it inserts under, so each level carries a
//! maximum and evicts its least recently written record once that maximum is passed
//! ([`MAX_BROWSERS`], [`MAX_BROWSER_SESSIONS`],
//! [`MAX_IDEMPOTENCY_RECORDS_PER_BROWSER`]).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as control_plane;

use crate::ops::types::{
    BrowserEnterprisePolicy, BrowserExtension, BrowserProfileConfiguration, Certificate,
    ProxyConfiguration, StartBrowserSessionInput, ToolsFileSystemConfiguration, ViewPort,
};

use super::{
    least_recently_written, ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState,
    SessionLocator,
};

/// The most browsers this emulator retains sessions for; the least recently used one is
/// evicted once the store passes it. An EMULATOR ceiling, not an AWS quota: a caller
/// names the `browserIdentifier` every start inserts under.
pub const MAX_BROWSERS: usize = 32;

/// The most sessions this emulator retains for one browser.
pub const MAX_BROWSER_SESSIONS: usize = 256;

/// The most `clientToken` records this emulator retains for one browser.
pub const MAX_IDEMPOTENCY_RECORDS_PER_BROWSER: usize = 1_000;

/// `BrowserSessionStatus` for a session that is still inside its time-to-live, so it can
/// serve work (source: smithy_query query="shape" shape="BrowserSessionStatus", values
/// READY | TERMINATED).
pub const BROWSER_SESSION_READY: &str = "READY";

/// `BrowserSessionStatus` for a session whose time-to-live has run out, from the same
/// enum.
pub const BROWSER_SESSION_TERMINATED: &str = "TERMINATED";

/// One browser's sessions, as `StartBrowserSession` wrote them.
#[derive(Debug, Default)]
pub struct BrowserStore {
    /// Sessions keyed by their minted `sessionId`. Bounded by
    /// [`MAX_BROWSER_SESSIONS`].
    pub sessions: HashMap<String, BrowserSessionRecord>,
    /// `clientToken` -> the session that token already started. `StartBrowserSession` is
    /// `@idempotent` and its `clientToken` is an `@idempotencyToken`, so a replayed
    /// token answers with the first session rather than starting a second one. Bounded
    /// by [`MAX_IDEMPOTENCY_RECORDS_PER_BROWSER`], and a record is dropped with the
    /// session it points at.
    pub idempotency: HashMap<String, SessionLocator>,
    /// The sequence of the last session started on this browser, which is what its
    /// eviction order is read from.
    pub last_seq: u64,
}

/// One browser session: what `StartBrowserSession` persisted, which is what
/// `GetBrowserSession` reports.
///
/// Every configuration member the start accepted is kept as the request asked for it,
/// because the read reports each one back; nothing here is defaulted to a value AWS does
/// not publish.
#[derive(Debug, Clone)]
pub struct BrowserSessionRecord {
    /// Minted by [`mint_browser_session_id`]; never client-supplied.
    pub session_id: String,
    pub browser_identifier: String,
    pub name: Option<String>,
    pub created_at: SystemTime,
    /// The session's time-to-live in seconds, as the request asked for it (the model
    /// defaults it to 3600 when a request omits it).
    pub session_timeout_seconds: i32,
    /// The viewport the start asked for, or `None` when it asked for none: AWS
    /// publishes no default viewport size, so this emulator reports no dimensions
    /// rather than dimensions of its own choosing.
    pub view_port: Option<ViewPort>,
    pub extensions: Option<Vec<BrowserExtension>>,
    pub enterprise_policies: Option<Vec<BrowserEnterprisePolicy>>,
    pub profile_configuration: Option<BrowserProfileConfiguration>,
    pub proxy_configuration: Option<ProxyConfiguration>,
    pub certificates: Option<Vec<Certificate>>,
    pub filesystem_configurations: Option<Vec<ToolsFileSystemConfiguration>>,
    /// When this session was last changed by a call: its creation instant until a call
    /// changes it.
    pub last_updated_at: SystemTime,
    /// The value [`ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::next_seq`]
    /// handed this session: its eviction order, and what keeps two sessions started in
    /// the same millisecond apart.
    pub seq: u64,
}

impl BrowserSessionRecord {
    /// A session as the call that starts it leaves it: the configuration that call asked
    /// for, created at `created_at`, nothing stopped and nothing changed since.
    pub fn started(seq: u64, created_at: SystemTime, input: &StartBrowserSessionInput) -> Self {
        Self {
            session_id: mint_browser_session_id(seq),
            browser_identifier: input.browser_identifier.clone(),
            name: input.name.clone(),
            created_at,
            session_timeout_seconds: input.session_timeout_seconds,
            view_port: input.view_port.clone(),
            extensions: input.extensions.clone(),
            enterprise_policies: input.enterprise_policies.clone(),
            profile_configuration: input.profile_configuration.clone(),
            proxy_configuration: input.proxy_configuration.clone(),
            certificates: input.certificates.clone(),
            filesystem_configurations: input.filesystem_configurations.clone(),
            // Nothing has changed this session yet, so the last instant it changed is
            // the instant it came into being.
            last_updated_at: created_at,
            seq,
        }
    }

    /// The instant this session's time-to-live runs out, or `None` when that instant is
    /// not representable (so nothing can be past it).
    ///
    /// `sessionTimeoutSeconds` is modelled as "the duration in seconds (time-to-live)
    /// after which the session automatically terminates, regardless of ongoing
    /// activity", so the session's own requested value is the span — nothing chosen
    /// here. The model's range makes it positive; a value that is not is read as no time
    /// at all rather than as a time in the past.
    pub fn expires_at(&self) -> Option<SystemTime> {
        let ttl = Duration::from_secs(u64::try_from(self.session_timeout_seconds).unwrap_or(0));
        self.created_at.checked_add(ttl)
    }

    /// This session's `BrowserSessionStatus` at `now`: `TERMINATED` once its
    /// time-to-live has run out, `READY` until then. The reading comes off stored state
    /// and the clock, so a read reports the termination that was already due without
    /// writing anything.
    pub fn status_as_of(&self, now: SystemTime) -> &'static str {
        match self.expires_at() {
            Some(expiry) if now >= expiry => BROWSER_SESSION_TERMINATED,
            _ => BROWSER_SESSION_READY,
        }
    }
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The stored browser session addressed by (browser, session), or `None` when this
    /// emulator holds no such session.
    pub fn browser_session(
        &self,
        browser_identifier: &str,
        session_id: &str,
    ) -> Option<&BrowserSessionRecord> {
        self.browsers
            .get(browser_identifier)?
            .sessions
            .get(session_id)
    }

    /// The stored browser session addressed by (browser, session), for a caller that is
    /// about to CHANGE it.
    pub fn browser_session_mut(
        &mut self,
        browser_identifier: &str,
        session_id: &str,
    ) -> Option<&mut BrowserSessionRecord> {
        self.browsers
            .get_mut(browser_identifier)?
            .sessions
            .get_mut(session_id)
    }

    /// The session a `clientToken` already started on this browser, or `None` when that
    /// token started none (or the session it started is no longer held).
    pub fn browser_session_for_token(
        &self,
        browser_identifier: &str,
        client_token: &str,
    ) -> Option<&BrowserSessionRecord> {
        let browser = self.browsers.get(browser_identifier)?;
        let session_id = &browser.idempotency.get(client_token)?.session_id;
        browser.sessions.get(session_id)
    }

    /// Persist one browser session — registering its browser on the way — and hold every
    /// bound this plane declares, so no caller can grow the store without limit.
    pub fn insert_browser_session(
        &mut self,
        session: BrowserSessionRecord,
        client_token: Option<String>,
    ) {
        let seq = session.seq;
        let session_id = session.session_id.clone();
        let browser = self
            .browsers
            .entry(session.browser_identifier.clone())
            .or_default();
        browser.last_seq = seq;
        browser.sessions.insert(session_id.clone(), session);
        if let Some(token) = client_token {
            browser
                .idempotency
                .insert(token, SessionLocator { session_id, seq });
            while browser.idempotency.len() > MAX_IDEMPOTENCY_RECORDS_PER_BROWSER {
                let Some(oldest) =
                    least_recently_written(&browser.idempotency, |locator| locator.seq)
                else {
                    break;
                };
                browser.idempotency.remove(&oldest);
            }
        }

        while browser.sessions.len() > MAX_BROWSER_SESSIONS {
            let Some(oldest) = least_recently_written(&browser.sessions, |session| session.seq)
            else {
                break;
            };
            browser.sessions.remove(&oldest);
            browser
                .idempotency
                .retain(|_, locator| locator.session_id != oldest);
        }
        while self.browsers.len() > MAX_BROWSERS {
            let Some(oldest) = least_recently_written(&self.browsers, |browser| browser.last_seq)
            else {
                break;
            };
            self.browsers.remove(&oldest);
        }
    }
}

/// Mint a `BrowserSessionId`, in the form
/// [`super::mint_alphanumeric_session_id`] states for that shape's pattern
/// (`^[0-9a-zA-Z]{1,40}$`, which `CodeInterpreterSessionId` publishes too).
pub fn mint_browser_session_id(seq: u64) -> String {
    super::mint_alphanumeric_session_id(seq)
}

/// Whether the browser a request names is one this emulator holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAvailability {
    /// The browser exists: the tool the service provides, or one the control plane
    /// created and has not removed.
    Held,
    /// No browser of that identifier exists, so nothing can be started on it.
    Unknown,
    /// The control plane's own store could not be reached, which is this emulator's
    /// fault and not the request's.
    StoreUnavailable,
}

/// Whether `browser_identifier` names a browser, read off the component that OWNS
/// browsers.
///
/// A browser is NOT this data plane's resource: the service-provided browser tool
/// (`aws.browser.v1`) is held by every account without anything creating it, and a custom
/// browser is created, reported and deleted through the AgentCore control plane
/// (`CreateBrowser` / `GetBrowser` / `DeleteBrowser`). So a session can only be started on
/// a browser that control plane returns from its own read — an identifier naming none is
/// an identifier naming nothing, whatever it looks like.
///
/// A browser's teardown completes inside the control plane's `DeleteBrowser` call, so its
/// store holds exactly the browsers that exist at this instant: this answers what
/// `GetBrowser` would answer at the same instant, with no transition left to materialise.
///
/// WHY THE OWNER'S STORE AND NOT THE SHARED MIRROR: the sibling code interpreter plane
/// asks `service_common::agentcore_resources::AgentCoreResourceInventory`, a registry the
/// control plane writes as it creates and deletes. That inventory has no browser half,
/// and a registry entry is not the owner's read: a browser the owner's store no longer
/// holds is the answer a session has to be refused on. Converging the two planes on one
/// idiom means giving the inventory that reach, not adding a third mechanism here.
pub fn browser_availability<Eng: Send + Sync + 'static>(
    engine: &Eng,
    browser_identifier: &str,
) -> BrowserAvailability {
    let Some(control) = control_plane::state::service_state(engine) else {
        return BrowserAvailability::StoreUnavailable;
    };
    if browser_identifier == control_plane::state::SYSTEM_BROWSER_ID {
        return BrowserAvailability::Held;
    }
    let store = control.lock();
    if store.browsers.contains_key(browser_identifier) {
        BrowserAvailability::Held
    } else {
        BrowserAvailability::Unknown
    }
}

/// Why a call addressed to a browser cannot be served, with the sentence it is refused
/// with: the ONE home of both messages, so the operations that resolve a browser cannot
/// word the same condition differently. Which modelled error carries the sentence is the
/// operation's own choice, because each one's error channel names its own variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserUnavailable {
    /// The identifier names no browser: the modelled `ResourceNotFoundException`, worded
    /// as this service words a lookup that missed
    /// ([`super::resource_not_found_message`]).
    NotFound(String),
    /// The component that owns browsers could not be reached: a modelled
    /// `InternalServerException`, which is this emulator's fault and not the request's.
    StoreUnavailable(String),
}

/// `Ok(())` when `browser_identifier` names a browser this emulator holds, or why it
/// does not and the sentence that refusal carries ([`browser_availability`] with the
/// wording attached).
pub fn resolve_browser<Eng: Send + Sync + 'static>(
    engine: &Eng,
    browser_identifier: &str,
) -> Result<(), BrowserUnavailable> {
    match browser_availability(engine, browser_identifier) {
        BrowserAvailability::Held => Ok(()),
        BrowserAvailability::Unknown => Err(BrowserUnavailable::NotFound(
            super::resource_not_found_message(
                super::NotFoundResource::Browser,
                browser_identifier,
            ),
        )),
        BrowserAvailability::StoreUnavailable => Err(BrowserUnavailable::StoreUnavailable(
            BROWSER_STORE_UNAVAILABLE.to_string(),
        )),
    }
}

/// The `InternalServerException` message a call gets when the component that owns
/// browsers could not be reached.
pub const BROWSER_STORE_UNAVAILABLE: &str = "the browser store is unavailable";
