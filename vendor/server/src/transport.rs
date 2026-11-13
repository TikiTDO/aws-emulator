// SPDX-License-Identifier: CC0-1.0

//! Optional framed payload exchanges. Ordinary service calls keep their buffered API.
//! No store, service, or credential authority is conferred by transport admission.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use http_body::{Body, Frame, SizeHint};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

pub type TransportError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type ExchangeFuture = Pin<Box<dyn Future<Output = Response<TransportBody>> + Send + 'static>>;

/// Send, not necessarily Sync: the HTTP driver polls each body exclusively.
pub struct TransportBody(UnsyncBoxBody<Bytes, TransportError>);

impl TransportBody {
    pub fn new<B>(body: B) -> Self
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<TransportError>,
    {
        Self(body.map_err(Into::into).boxed_unsync())
    }
}

impl From<Bytes> for TransportBody {
    fn from(bytes: Bytes) -> Self {
        Self::new(Full::new(bytes))
    }
}

impl Body for TransportBody {
    type Data = Bytes;
    type Error = TransportError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        Pin::new(&mut self.0).poll_frame(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}

pub enum TransportSelection {
    Buffered,
    Refused(Response<Bytes>),
    Payload(Arc<dyn PayloadExchange>),
}

/// An owner error mapper must not accidentally turn refusal into success/redirect.
pub fn normalize_refusal(mut response: Response<Bytes>) -> Response<Bytes> {
    if response.status().as_u16() < 400 {
        *response.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
        response.headers_mut().remove(http::header::LOCATION);
    }
    response
}

pub trait PayloadExchange: Send + Sync + 'static {
    fn run(
        self: Arc<Self>,
        request: Request<TransportBody>,
        context: ExchangeContext,
    ) -> ExchangeFuture;
}

/// Finite per-exchange defaults; a service can impose stricter quotas independently.
/// These are transport bounds, not AWS service limits or a storage retention policy.
#[derive(Clone, Debug)]
pub struct ExchangeLimits {
    pub max_entity_bytes: u64,
    pub max_frame_bytes: usize,
    pub progress_timeout: Duration,
}

impl Default for ExchangeLimits {
    fn default() -> Self {
        Self {
            max_entity_bytes: 8 * 1024 * 1024 * 1024,
            max_frame_bytes: 1024 * 1024,
            progress_timeout: Duration::from_secs(60),
        }
    }
}

struct Lifetime {
    permit: std::sync::Mutex<Option<OwnedSemaphorePermit>>,
    cancelled: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct ExchangeContext {
    lifetime: Arc<Lifetime>,
    limits: ExchangeLimits,
}

impl ExchangeContext {
    pub fn limits(&self) -> &ExchangeLimits {
        &self.limits
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.lifetime.cancelled.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
    pub async fn until_cancelled<F: Future>(&self, future: F) -> Result<F::Output, TransportError> {
        tokio::select! {
            _ = self.cancelled() => Err(error("payload exchange cancelled")),
            result = tokio::time::timeout(self.limits.progress_timeout, future) => result.map_err(|_| error("payload progress deadline exceeded")),
        }
    }
    pub fn is_cancelled(&self) -> bool {
        *self.lifetime.cancelled.borrow()
    }
    fn cancel(&self) {
        self.lifetime.cancelled.send_replace(true);
        if let Ok(mut permit) = self.lifetime.permit.lock() {
            permit.take();
        }
    }
}

/// Process-selected finite limits. Invalid configuration is a refusal, not a fallback.
/// The network boot profile and durable store quotas remain distinct owners.
pub struct TransportPolicy {
    pub limits: ExchangeLimits,
    pub active_transfers: usize,
    pub waiting_transfers: usize,
}
impl TransportPolicy {
    pub fn from_environment() -> Result<Self, &'static str> {
        fn number(name: &str, default: u64) -> Result<u64, &'static str> {
            match std::env::var(name) {
                Err(std::env::VarError::NotPresent) => Ok(default),
                Ok(value) => value
                    .parse::<u64>()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or("invalid payload transport limit"),
                Err(_) => Err("invalid payload transport limit"),
            }
        }
        let frame = usize::try_from(number("EMULATOR_PAYLOAD_MAX_FRAME_BYTES", 1024 * 1024)?)
            .map_err(|_| "payload frame limit exceeds host size")?;
        let active = usize::try_from(number("EMULATOR_PAYLOAD_ACTIVE_TRANSFERS", 8)?)
            .ok()
            .filter(|n| *n <= Semaphore::MAX_PERMITS)
            .ok_or("invalid active transfer limit")?;
        let waiting = usize::try_from(number("EMULATOR_PAYLOAD_WAITING_TRANSFERS", 16)?)
            .ok()
            .filter(|n| *n <= Semaphore::MAX_PERMITS)
            .ok_or("invalid waiting transfer limit")?;
        let seconds = number("EMULATOR_PAYLOAD_PROGRESS_SECONDS", 60)?;
        if seconds > 86400 {
            return Err("payload progress deadline exceeds one day");
        }
        Ok(Self {
            limits: ExchangeLimits {
                max_entity_bytes: number(
                    "EMULATOR_PAYLOAD_MAX_ENTITY_BYTES",
                    8 * 1024 * 1024 * 1024,
                )?,
                max_frame_bytes: frame,
                progress_timeout: Duration::from_secs(seconds),
            },
            active_transfers: active,
            waiting_transfers: waiting,
        })
    }
}
struct Admission {
    active: Arc<Semaphore>,
    waiters: Semaphore,
    limits: ExchangeLimits,
}
fn admission() -> Result<&'static Admission, &'static str> {
    static ADMISSION: OnceLock<Result<Admission, &'static str>> = OnceLock::new();
    ADMISSION
        .get_or_init(|| {
            let policy = TransportPolicy::from_environment()?;
            Ok(Admission {
                active: Arc::new(Semaphore::new(policy.active_transfers)),
                waiters: Semaphore::new(policy.waiting_transfers),
                limits: policy.limits,
            })
        })
        .as_ref()
        .map_err(|error| *error)
}

/// Freeze and validate the process's transport budgets before opening listeners.
/// Standalone boot calls this explicitly; embeddings retain lazy first-use selection.
/// Both paths use the same retained result, including a configuration refusal.
pub fn prepare() -> Result<(), &'static str> {
    admission().map(|_| ())
}

#[derive(Default)]
struct ScopeState {
    closed: bool,
    active: Vec<Weak<Lifetime>>,
}
/// One listener's exchanges, not a process-wide shutdown switch for unrelated embedders.
#[derive(Clone, Default)]
pub struct ExchangeScope(Arc<std::sync::Mutex<ScopeState>>);
impl ExchangeScope {
    fn register(&self, lifetime: &Arc<Lifetime>) -> Result<(), TransportError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| error("exchange scope poisoned"))?;
        if state.closed {
            return Err(error("listener is shutting down"));
        }
        state.active.retain(|weak| weak.strong_count() > 0);
        state.active.push(Arc::downgrade(lifetime));
        Ok(())
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.closed = true;
            for lifetime in state.active.drain(..).filter_map(|weak| weak.upgrade()) {
                lifetime.cancelled.send_replace(true);
                // Cancellation wakes the owner; its dropped/completed exchange releases
                // admission only when the actual request/body lifetime ends.
            }
        }
    }
    pub fn guard(&self) -> ExchangeScopeGuard {
        ExchangeScopeGuard(self.clone())
    }
}
pub struct ExchangeScopeGuard(ExchangeScope);
impl Drop for ExchangeScopeGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn error(message: &'static str) -> TransportError {
    Box::new(std::io::Error::other(message))
}

fn refusal(status: http::StatusCode) -> Response<TransportBody> {
    let mut response = Response::new(TransportBody::from(Bytes::new()));
    *response.status_mut() = status;
    response
}

/// Carries cancellation across a dropped request future and then the response body.
struct CancelOnDrop(Option<ExchangeContext>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(context) = &self.0 {
            context.cancel();
        }
    }
}

/// A body whose retained frames, total ingress and progress are bounded. It also contains
/// panics AFTER response headers; those become terminal body errors, never a new response.
struct GuardedBody {
    inner: TransportBody,
    context: ExchangeContext,
    deadline: Pin<Box<tokio::time::Sleep>>,
    cancellation: Pin<Box<dyn Future<Output = ()> + Send>>,
    count: u64,
    ingress: bool,
    finished: bool,
    cancel_on_drop: bool,
}

impl GuardedBody {
    fn new(inner: TransportBody, context: ExchangeContext, ingress: bool) -> Self {
        let deadline = Box::pin(tokio::time::sleep(context.limits.progress_timeout));
        let cancelled_context = context.clone();
        let cancellation = Box::pin(async move { cancelled_context.cancelled().await });
        Self {
            inner,
            context,
            deadline,
            cancellation,
            count: 0,
            ingress,
            finished: false,
            cancel_on_drop: !ingress,
        }
    }
    fn fail(
        &mut self,
        message: &'static str,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        self.finished = true;
        if !self.ingress {
            self.context.cancel();
        }
        Poll::Ready(Some(Err(error(message))))
    }
}

impl Drop for GuardedBody {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.context.cancel();
        }
    }
}

impl Body for GuardedBody {
    type Data = Bytes;
    type Error = TransportError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        if self.finished {
            return Poll::Ready(None);
        }
        if self.cancellation.as_mut().poll(cx).is_ready() {
            return self.fail("payload exchange cancelled");
        }
        if self.deadline.as_mut().poll(cx).is_ready() {
            return self.fail("payload progress deadline exceeded");
        }
        let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Pin::new(&mut self.inner).poll_frame(cx)
        }));
        match polled {
            Err(_) => {
                tracing::error!("contained panic while polling payload body");
                self.fail("payload body panicked")
            }
            Ok(Poll::Ready(Some(Ok(frame)))) => {
                if let Some(data) = frame.data_ref() {
                    if data.len() > self.context.limits.max_frame_bytes {
                        return self.fail("payload frame exceeds buffer budget");
                    }
                    let Some(count) = self.count.checked_add(data.len() as u64) else {
                        return self.fail("payload byte count overflow");
                    };
                    if self.ingress && count > self.context.limits.max_entity_bytes {
                        return self.fail("payload entity exceeds limit");
                    }
                    self.count = count;
                    if !data.is_empty() {
                        let next =
                            tokio::time::Instant::now() + self.context.limits.progress_timeout;
                        self.deadline.as_mut().reset(next);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Ok(Poll::Ready(None)) => {
                self.finished = true;
                if !self.ingress {
                    self.context.cancel();
                }
                Poll::Ready(None)
            }
            Ok(Poll::Ready(Some(Err(cause)))) => {
                self.finished = true;
                if !self.ingress {
                    self.context.cancel();
                }
                Poll::Ready(Some(Err(cause)))
            }
            Ok(Poll::Pending) => Poll::Pending,
        }
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
    fn is_end_stream(&self) -> bool {
        self.finished
    }
}

/// Common production/embedded driver; never collects a selected payload.
pub async fn execute(
    exchange: Arc<dyn PayloadExchange>,
    request: Request<TransportBody>,
) -> Response<TransportBody> {
    let admission = match admission() {
        Ok(admission) => admission,
        Err(_) => return refusal(http::StatusCode::SERVICE_UNAVAILABLE),
    };
    let limits = admission.limits.clone();
    let scope = request
        .extensions()
        .get::<ExchangeScope>()
        .cloned()
        .unwrap_or_default();
    let Ok(waiter) = admission.waiters.try_acquire() else {
        return refusal(http::StatusCode::SERVICE_UNAVAILABLE);
    };
    let permit = match tokio::time::timeout(
        limits.progress_timeout,
        admission.active.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => return refusal(http::StatusCode::SERVICE_UNAVAILABLE),
    };
    drop(waiter);
    let (cancelled, _) = watch::channel(false);
    let context = ExchangeContext {
        lifetime: Arc::new(Lifetime {
            permit: std::sync::Mutex::new(Some(permit)),
            cancelled,
        }),
        limits,
    };
    if scope.register(&context.lifetime).is_err() {
        return refusal(http::StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut cancellation = CancelOnDrop(Some(context.clone()));
    let facts = crate::RequestFacts::from_request(&request);
    let (parts, body) = request.into_parts();
    let request = Request::from_parts(
        parts,
        TransportBody::new(GuardedBody::new(body, context.clone(), true)),
    );
    let run_context = context.clone();
    let response = crate::contain_panics(
        facts,
        async move { exchange.run(request, run_context).await },
    )
    .await;
    let (parts, body) = response.into_parts();
    let body = TransportBody::new(GuardedBody::new(body, context, false));
    cancellation.0.take();
    Response::from_parts(parts, body)
}

/// SDK bodies require Sync; this shim adds it without collecting. Polling has exclusive
/// access; shared metadata reads hold a synchronous lock only for the duration of a call.
pub struct SdkTransportBody(std::sync::Mutex<TransportBody>);
impl SdkTransportBody {
    pub fn new(body: TransportBody) -> Self {
        Self(std::sync::Mutex::new(body))
    }
}
impl Body for SdkTransportBody {
    type Data = Bytes;
    type Error = TransportError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransportError>>> {
        match self.0.get_mut() {
            Ok(body) => Pin::new(body).poll_frame(cx),
            Err(_) => Poll::Ready(Some(Err(error("payload adapter lock poisoned")))),
        }
    }
    fn is_end_stream(&self) -> bool {
        self.0
            .lock()
            .map(|body| body.is_end_stream())
            .unwrap_or(false)
    }
    fn size_hint(&self) -> SizeHint {
        self.0
            .lock()
            .map(|body| body.size_hint())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Frames(VecDeque<Frame<Bytes>>);
    impl Body for Frames {
        type Data = Bytes;
        type Error = std::convert::Infallible;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
            Poll::Ready(self.0.pop_front().map(Ok))
        }
    }
    struct Echo;
    impl PayloadExchange for Echo {
        fn run(
            self: Arc<Self>,
            request: Request<TransportBody>,
            _: ExchangeContext,
        ) -> ExchangeFuture {
            Box::pin(async move { Response::new(request.into_body()) })
        }
    }

    #[tokio::test]
    async fn framed_exchange_preserves_data_and_trailers_through_sdk_adapter() {
        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-fixture", http::HeaderValue::from_static("done"));
        let input = Frames(VecDeque::from([
            Frame::data(Bytes::from_static(b"first")),
            Frame::data(Bytes::from_static(b"second")),
            Frame::trailers(trailers.clone()),
        ]));
        let response = execute(Arc::new(Echo), Request::new(TransportBody::new(input))).await;
        let mut body = aws_smithy_types::body::SdkBody::from_body_1_x(SdkTransportBody::new(
            response.into_body(),
        ));
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "first"
        );
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "second"
        );
        assert_eq!(
            body.frame()
                .await
                .unwrap()
                .unwrap()
                .into_trailers()
                .unwrap(),
            trailers
        );
        assert!(body.frame().await.is_none());
    }

    struct WatchDrop(Arc<AtomicBool>);
    impl Drop for WatchDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl Body for WatchDrop {
        type Data = Bytes;
        type Error = std::convert::Infallible;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
            Poll::Pending
        }
    }

    #[tokio::test]
    async fn response_drop_releases_its_body_and_exchange_lease() {
        let dropped = Arc::new(AtomicBool::new(false));
        let response = execute(
            Arc::new(Echo),
            Request::new(TransportBody::new(WatchDrop(dropped.clone()))),
        )
        .await;
        assert!(!dropped.load(Ordering::SeqCst));
        drop(response);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn listener_scope_cancels_only_its_own_stream() {
        let scope = ExchangeScope::default();
        let mut request = Request::new(TransportBody::new(WatchDrop(Arc::new(AtomicBool::new(
            false,
        )))));
        request.extensions_mut().insert(scope.clone());
        let mut own = execute(Arc::new(Echo), request).await.into_body();
        let mut other = execute(
            Arc::new(Echo),
            Request::new(TransportBody::from(Bytes::from_static(b"other"))),
        )
        .await
        .into_body();
        scope.close();
        assert!(own.frame().await.unwrap().is_err());
        assert_eq!(
            other.frame().await.unwrap().unwrap().into_data().unwrap(),
            "other"
        );
    }

    struct PanicsOnPoll;
    impl Body for PanicsOnPoll {
        type Data = Bytes;
        type Error = std::convert::Infallible;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
            panic!("fixture body panic")
        }
    }
    #[tokio::test]
    async fn body_poll_panics_are_terminal_errors_not_empty_success() {
        let mut body = execute(
            Arc::new(Echo),
            Request::new(TransportBody::new(PanicsOnPoll)),
        )
        .await
        .into_body();
        assert!(body.frame().await.unwrap().is_err());
        assert!(body.frame().await.is_none());
    }

    #[derive(Clone)]
    struct SelectedEcho;
    impl crate::EmulatorService for SelectedEcho {
        fn select_transport(&self, _: &http::request::Parts) -> TransportSelection {
            TransportSelection::Payload(Arc::new(Echo))
        }
        async fn handle_request(&self, _: Request<Bytes>) -> Response<Bytes> {
            panic!("selected route reached buffered handler")
        }
    }
    #[tokio::test]
    async fn sdk_connector_preserves_selected_stream_instead_of_empty_response() {
        let mut request = aws_smithy_runtime_api::client::orchestrator::HttpRequest::get(
            "http://localhost/payload",
        )
        .unwrap();
        *request.body_mut() = aws_smithy_types::body::SdkBody::from_body_1_x(
            SdkTransportBody::new(TransportBody::new(Frames(VecDeque::from([
                Frame::data(Bytes::from_static(b"one")),
                Frame::data(Bytes::from_static(b"two")),
            ])))),
        );
        let response = crate::http_connector_call(&SelectedEcho, request)
            .await
            .unwrap();
        let mut body = response.into_body();
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "one"
        );
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "two"
        );
        assert!(body.frame().await.is_none());
    }
    #[test]
    fn wrapper_predicates_keep_precedence_over_selected_payloads() {
        use crate::EmulatorService;
        let wrapped = crate::WithExtraRoutes::new(SelectedEcho).route_matching(
            |request| !request.body().is_empty(),
            |_request| async { Response::new(Bytes::new()) },
        );
        let (parts, _) = Request::new(()).into_parts();
        assert!(matches!(
            wrapped.select_transport(&parts),
            TransportSelection::Buffered
        ));
        let nonmatching = crate::WithExtraRoutes::new(SelectedEcho).route(
            http::Method::GET,
            "/health",
            |_request| async { Response::new(Bytes::new()) },
        );
        let (parts, _) = Request::builder()
            .uri("/payload")
            .body(())
            .unwrap()
            .into_parts();
        assert!(matches!(
            nonmatching.select_transport(&parts),
            TransportSelection::Payload(_)
        ));
    }
}
