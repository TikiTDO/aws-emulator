// SPDX-License-Identifier: CC0-1.0

//! Service-owned payload semantics behind a private-body, terminal-verification driver.
//! This module owns no disk paths or store. A receipt is evidence about decoded bytes,
//! not service authorization, storage durability or publication.

use crate::BoxFuture;
use bytes::Bytes;
use emulator_server::request_authentication::{
    RequestAuthenticationStream, StreamAuthentication, StreamAuthenticationError,
    StreamValidationError,
};
use emulator_server::transport::{
    ExchangeContext, ExchangeFuture, PayloadExchange, TransportBody, TransportError,
};
use http::{Request, Response, request::Parts};
use http_body_util::BodyExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub enum PayloadOperation {
    /// Must match the header-addressable modeled routing key and its auth schemes.
    Modeled(&'static str),
    /// An explicitly service-owned native protocol; never overrides a modeled operation.
    Native(&'static str),
}

pub struct PayloadRoute {
    pub operation: PayloadOperation,
    pub handler: Arc<dyn PayloadHandler>,
}

/// The service supplies all policy and error semantics. The runtime supplies framing and
/// signature validation. Implementations hold their own engine/store handles.
pub trait PayloadHandler: Send + Sync + 'static {
    /// Required even for native routes: Basic/capability/policy checks belong to the owner.
    /// A success here does not validate the body or permit final publication.
    fn authorize(&self, original: &Parts) -> Result<(), Response<Bytes>>;
    fn refuse(&self, error: PayloadReceiveError) -> Response<Bytes>;
    fn run(self: Arc<Self>, request: PayloadRequest, context: ExchangeContext) -> ExchangeFuture;
}

/// Append only to private staging; successful completion of this method is not commit.
pub trait PayloadSink: Send {
    fn append<'a>(&'a mut self, bytes: Bytes) -> BoxFuture<'a, Result<(), TransportError>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadAdmission {
    Signed,
    ModelUnsigned,
    AuthenticationDisabled,
    NativeOwner,
}

pub enum PayloadReceiveError {
    Begin(StreamAuthenticationError),
    Validation(StreamValidationError),
    Transport(TransportError),
    Sink(TransportError),
    Framing(&'static str),
}

impl std::fmt::Debug for PayloadReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render signature metadata or owner-provided sink error contents by default.
        f.write_str(match self {
            Self::Begin(_) => "PayloadBeginRefused",
            Self::Validation(_) => "PayloadValidationFailed",
            Self::Transport(_) => "PayloadTransportFailed",
            Self::Sink(_) => "PayloadSinkFailed",
            Self::Framing(_) => "PayloadFramingFailed",
        })
    }
}

/// Private construction: produced only after true body EOF and successful verifier finish.
pub struct PayloadReceipt {
    validation: StreamAuthentication,
    admission: PayloadAdmission,
}
impl PayloadReceipt {
    pub fn validation(&self) -> &StreamAuthentication {
        &self.validation
    }
    pub fn admission(&self) -> PayloadAdmission {
        self.admission
    }
}

pub struct PayloadRequest {
    parts: Parts,
    body: TransportBody,
    verifier: Box<dyn RequestAuthenticationStream>,
    admission: PayloadAdmission,
    validated: Arc<AtomicBool>,
}

impl PayloadRequest {
    pub fn parts(&self) -> &Parts {
        &self.parts
    }

    /// Feed each raw entity byte exactly once; await the private stage before advancing.
    /// Neither raw body nor verifier is exposed, so a caller cannot finalize after a prefix.
    pub async fn receive_into(
        self,
        sink: &mut dyn PayloadSink,
        context: &ExchangeContext,
    ) -> Result<PayloadReceipt, PayloadReceiveError> {
        let Self {
            parts,
            mut body,
            mut verifier,
            admission,
            validated,
        } = self;
        let lengths: Vec<_> = parts
            .headers
            .get_all(http::header::CONTENT_LENGTH)
            .iter()
            .collect();
        if lengths.len() > 1 {
            return Err(PayloadReceiveError::Framing("duplicate Content-Length"));
        }
        let stated_length = match lengths.first() {
            Some(value) => Some(
                value
                    .to_str()
                    .ok()
                    .filter(|value| {
                        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or(PayloadReceiveError::Framing("invalid Content-Length"))?,
            ),
            None => None,
        };
        let mut entity_bytes = 0u64;
        let mut trailers = None;
        loop {
            let frame = context
                .until_cancelled(body.frame())
                .await
                .map_err(PayloadReceiveError::Transport)?;
            let Some(frame) = frame else {
                break;
            };
            let frame = frame.map_err(PayloadReceiveError::Transport)?;
            match frame.into_data() {
                Ok(bytes) => {
                    if trailers.is_some() {
                        return Err(PayloadReceiveError::Framing("data after HTTP trailers"));
                    }
                    entity_bytes = entity_bytes
                        .checked_add(bytes.len() as u64)
                        .ok_or(PayloadReceiveError::Framing("entity length overflow"))?;
                    let mut offset = 0;
                    while offset < bytes.len() {
                        let step = verifier
                            .consume(&bytes[offset..])
                            .map_err(PayloadReceiveError::Validation)?;
                        if step.consumed == 0 || step.consumed > bytes.len() - offset {
                            return Err(PayloadReceiveError::Framing(
                                "decoder made invalid progress",
                            ));
                        }
                        if let Some(payload) = step.payload {
                            // The decoder returns a subslice of this input frame. Validate that
                            // invariant before forming a zero-copy Bytes view for the async sink.
                            let start = (payload.as_ptr() as usize)
                                .checked_sub(bytes.as_ptr() as usize)
                                .ok_or(PayloadReceiveError::Framing(
                                    "decoder returned foreign bytes",
                                ))?;
                            let end = start
                                .checked_add(payload.len())
                                .filter(|end| *end <= offset + step.consumed)
                                .filter(|_| start >= offset)
                                .ok_or(PayloadReceiveError::Framing(
                                    "decoder returned bytes outside consumed span",
                                ))?;
                            if start != end {
                                context
                                    .until_cancelled(sink.append(bytes.slice(start..end)))
                                    .await
                                    .map_err(PayloadReceiveError::Transport)?
                                    .map_err(PayloadReceiveError::Sink)?;
                            }
                        }
                        offset += step.consumed;
                    }
                }
                Err(frame) => {
                    let headers = frame
                        .into_trailers()
                        .map_err(|_| PayloadReceiveError::Framing("unknown body frame"))?;
                    if trailers.replace(headers).is_some() {
                        return Err(PayloadReceiveError::Framing("duplicate HTTP trailers"));
                    }
                }
            }
        }
        if stated_length.is_some_and(|length| length != entity_bytes) {
            return Err(PayloadReceiveError::Framing(
                "Content-Length does not match entity",
            ));
        }
        if context.is_cancelled() {
            return Err(PayloadReceiveError::Framing(
                "request cancelled before validation",
            ));
        }
        let validation = verifier
            .finish(trailers.as_ref())
            .map_err(PayloadReceiveError::Validation)?;
        validated.store(true, Ordering::Release);
        Ok(PayloadReceipt {
            validation,
            admission,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Verification {
    Authenticator(&'static dyn emulator_server::RequestAuthenticator),
    Uncredentialed,
}

pub(crate) struct RuntimeExchange {
    pub handler: Arc<dyn PayloadHandler>,
    pub verification: Verification,
    pub credentials: Arc<emulator_server::CredentialStore>,
    pub signing_name: &'static str,
    pub admission: PayloadAdmission,
}

impl PayloadExchange for RuntimeExchange {
    fn run(
        self: Arc<Self>,
        request: Request<TransportBody>,
        context: ExchangeContext,
    ) -> ExchangeFuture {
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            if let Err(response) = self.handler.authorize(&parts) {
                return emulator_server::transport::normalize_refusal(response)
                    .map(TransportBody::from);
            }
            let begun = match self.verification {
                Verification::Authenticator(authenticator) => {
                    authenticator.begin_stream(&parts, self.signing_name, self.credentials.clone())
                }
                Verification::Uncredentialed => {
                    emulator_server::request_authentication::begin_uncredentialed_stream(&parts)
                }
            };
            let verifier = match begun {
                Ok(verifier) => verifier,
                Err(error) => {
                    return emulator_server::transport::normalize_refusal(
                        self.handler.refuse(PayloadReceiveError::Begin(error)),
                    )
                    .map(TransportBody::from);
                }
            };
            let validated = Arc::new(AtomicBool::new(false));
            let request = PayloadRequest {
                parts,
                body,
                verifier,
                admission: self.admission,
                validated: validated.clone(),
            };
            let response = self.handler.clone().run(request, context).await;
            if response.status().as_u16() < 400 && !validated.load(Ordering::Acquire) {
                return emulator_server::transport::normalize_refusal(self.handler.refuse(
                    PayloadReceiveError::Framing(
                        "handler returned success without terminal validation",
                    ),
                ))
                .map(TransportBody::from);
            }
            response
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Stage {
        bytes: usize,
    }
    impl PayloadSink for Stage {
        fn append<'a>(&'a mut self, bytes: Bytes) -> BoxFuture<'a, Result<(), TransportError>> {
            Box::pin(async move {
                self.bytes += bytes.len();
                Ok(())
            })
        }
    }
    struct Handler {
        consume: bool,
        committed: Arc<AtomicBool>,
        status: http::StatusCode,
    }
    impl PayloadHandler for Handler {
        fn authorize(&self, _: &Parts) -> Result<(), Response<Bytes>> {
            Ok(())
        }
        fn refuse(&self, _: PayloadReceiveError) -> Response<Bytes> {
            Response::builder()
                .status(400)
                .body(Bytes::from_static(b"refused"))
                .unwrap()
        }
        fn run(
            self: Arc<Self>,
            request: PayloadRequest,
            context: ExchangeContext,
        ) -> ExchangeFuture {
            Box::pin(async move {
                if self.consume {
                    let mut stage = Stage { bytes: 0 };
                    match request.receive_into(&mut stage, &context).await {
                        Ok(receipt) => {
                            assert_eq!(receipt.validation().decoded_bytes(), stage.bytes as u64);
                            self.committed.store(true, Ordering::SeqCst);
                        }
                        Err(error) => return self.refuse(error).map(TransportBody::from),
                    }
                }
                let mut response = Response::new(TransportBody::from(Bytes::new()));
                *response.status_mut() = self.status;
                response
            })
        }
    }
    fn exchange(consume: bool, committed: Arc<AtomicBool>) -> Arc<RuntimeExchange> {
        Arc::new(RuntimeExchange {
            handler: Arc::new(Handler {
                consume,
                committed,
                status: http::StatusCode::OK,
            }),
            verification: Verification::Uncredentialed,
            credentials: Arc::new(emulator_server::CredentialStore::default()),
            signing_name: "s3",
            admission: PayloadAdmission::AuthenticationDisabled,
        })
    }

    #[tokio::test]
    async fn successful_response_requires_actual_eof_validation() {
        let committed = Arc::new(AtomicBool::new(false));
        let response = emulator_server::transport::execute(
            exchange(false, committed.clone()),
            Request::new(TransportBody::from(Bytes::from_static(b"unread"))),
        )
        .await;
        assert_eq!(response.status(), 400);
        assert!(!committed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn truncated_entity_cannot_produce_a_publication_receipt() {
        for (length, expected) in [(3, true), (4, false)] {
            let committed = Arc::new(AtomicBool::new(false));
            let request = Request::builder()
                .header("content-length", length.to_string())
                .body(TransportBody::from(Bytes::from_static(b"abc")))
                .unwrap();
            let response =
                emulator_server::transport::execute(exchange(true, committed.clone()), request)
                    .await;
            assert_eq!(response.status().is_success(), expected);
            assert_eq!(committed.load(Ordering::SeqCst), expected);
        }
    }

    #[tokio::test]
    async fn redirect_cannot_publish_location_before_terminal_validation() {
        let exchange = Arc::new(RuntimeExchange {
            handler: Arc::new(Handler {
                consume: false,
                committed: Arc::new(AtomicBool::new(false)),
                status: http::StatusCode::TEMPORARY_REDIRECT,
            }),
            verification: Verification::Uncredentialed,
            credentials: Arc::new(emulator_server::CredentialStore::default()),
            signing_name: "s3",
            admission: PayloadAdmission::AuthenticationDisabled,
        });
        let response = emulator_server::transport::execute(
            exchange,
            Request::new(TransportBody::from(Bytes::new())),
        )
        .await;
        assert_eq!(response.status(), 400);
    }

    struct BrokenRefusal;
    impl PayloadHandler for BrokenRefusal {
        fn authorize(&self, _: &Parts) -> Result<(), Response<Bytes>> {
            Err(Response::new(Bytes::new()))
        }
        fn refuse(&self, _: PayloadReceiveError) -> Response<Bytes> {
            Response::new(Bytes::new())
        }
        fn run(self: Arc<Self>, _: PayloadRequest, _: ExchangeContext) -> ExchangeFuture {
            Box::pin(async { panic!("authorization refusal must not execute") })
        }
    }
    #[tokio::test]
    async fn erroneous_success_from_authorization_refusal_is_not_published() {
        let exchange = Arc::new(RuntimeExchange {
            handler: Arc::new(BrokenRefusal),
            verification: Verification::Uncredentialed,
            credentials: Arc::new(emulator_server::CredentialStore::default()),
            signing_name: "s3",
            admission: PayloadAdmission::AuthenticationDisabled,
        });
        let response = emulator_server::transport::execute(
            exchange,
            Request::new(TransportBody::from(Bytes::new())),
        )
        .await;
        assert_eq!(response.status(), 500);
    }
}
