// SPDX-License-Identifier: CC0-1.0

//! Capability adapter bridging `DetectFaces`, `CompareFaces`, and `DetectLabels` to an
//! external vision inference daemon when `EMULATOR_VISION_ENDPOINT` (or the engine's
//! `vision_endpoint` override) is configured.
//!
//! ## Transport and security boundary
//!
//! * **Opt-in preservation**: when no endpoint is configured, [`configured_endpoint`]
//!   returns `Ok(None)` and every operation runs the built-in pixel detector unchanged.
//! * **No caller-controlled filesystem paths**: images are resolved in-process via
//!   [`super::image_source::image_bytes`] (either inline `Bytes` or an emulated `S3Object`
//!   bounded to [`super::image_source::MAX_S3_IMAGE_BYTES`], 15 MiB) and transmitted as
//!   bounded raw `application/octet-stream` payloads over a Unix domain socket
//!   (`unix:///path/to/rekognition.sock`) or HTTP/1.1 TCP (`http://host:port`).
//! * **Honest failure**: if an endpoint IS configured and cannot be reached, times out, or
//!   answers with a server fault, the call fails honestly with
//!   [`super::image_source::UnreadableImage::NotAnalysed`] (`InternalServerError`) rather
//!   than inventing synthetic attributes.

use std::path::PathBuf;
use std::time::Duration;

use ::bytes::Bytes;
use ::http_body_util::{BodyExt, Full};
use ::hyper::client::conn::http1;
use ::hyper_util::rt::TokioIo;
use ::serde::Deserialize;
use ::tokio::net::{TcpStream, UnixStream};

use super::image_source::{MAX_S3_IMAGE_BYTES, UnreadableImage};
use super::label_detection::{self, ImageAnalysis, Requested};
use crate::ops::types::{
    AgeRange, Beard, BoundingBox, CompareFacesMatch, ComparedFace, ComparedSourceImageFace,
    DetectLabelsImageProperties, DetectLabelsImageQuality, DominantColor, Emotion, EyeDirection,
    EyeOpen, Eyeglasses, FaceDetail, FaceOccluded, Gender, ImageQuality, Instance, Label, Landmark,
    MouthOpen, Mustache, Parent, Pose, Smile, Sunglasses,
};

/// Environment variable naming the external vision inference endpoint
/// (`unix:///path/to/rekognition.sock` or `http://127.0.0.1:9299`).
pub const ENV_VISION_ENDPOINT: &str = "EMULATOR_VISION_ENDPOINT";

/// Per-request timeout covering socket connect, request write, inference, and body read.
const VISION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on JSON response payloads accepted from the vision provider (4 MiB).
const MAX_VISION_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Parsed transport endpoint for the external vision provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisionEndpoint {
    /// Same-host Unix domain socket (`unix:///path/to/sock` or `/path/to/sock`).
    Unix(PathBuf),
    /// HTTP/1.1 over TCP (`http://host:port[/base_path]`).
    Http {
        authority: String,
        connect_addr: String,
        base_path: String,
    },
}

impl VisionEndpoint {
    /// Parses an endpoint specification from `unix:///path`, `/path`, or `http://host[:port][/path]`.
    pub fn parse(raw: &str) -> Result<Self, UnreadableImage> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(UnreadableImage::NotAnalysed(
                "Vision provider endpoint is empty".to_string(),
            ));
        }
        if let Some(sock) = trimmed.strip_prefix("unix://") {
            if !sock.starts_with('/') {
                return Err(UnreadableImage::NotAnalysed(
                    "Unix vision endpoint must use an absolute socket path (unix:///path/to.sock)"
                        .to_string(),
                ));
            }
            return Ok(Self::Unix(PathBuf::from(sock)));
        }
        if trimmed.starts_with('/') {
            return Ok(Self::Unix(PathBuf::from(trimmed)));
        }
        let Some(rest) = trimmed.strip_prefix("http://") else {
            return Err(UnreadableImage::NotAnalysed(
                "Vision provider endpoint must start with unix:// or http://".to_string(),
            ));
        };
        let (authority, path_part) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };
        if authority.is_empty()
            || authority.contains(['?', '#', '@', ' '])
            || path_part.contains(['?', '#'])
        {
            return Err(UnreadableImage::NotAnalysed(
                "Vision provider HTTP endpoint is malformed".to_string(),
            ));
        }
        let connect_addr = if has_explicit_port(authority) {
            authority.to_string()
        } else {
            format!("{authority}:80")
        };
        let base_path = path_part.trim_end_matches('/').to_string();
        Ok(Self::Http {
            authority: authority.to_string(),
            connect_addr,
            base_path,
        })
    }

    /// Calls `POST /v1/rekognition/detect-faces` with bounded raw image bytes and maps
    /// the provider's response onto modelled `FaceDetail` records filtered by `attributes`.
    pub async fn detect_faces(
        &self,
        image_bytes: &[u8],
        attributes: Option<&[String]>,
    ) -> Result<Vec<FaceDetail>, UnreadableImage> {
        validate_image_ceiling(image_bytes)?;
        let requested = RequestedAttributes::from_slice(attributes);
        let path_and_query = format!(
            "/v1/rekognition/detect-faces?attributes={}",
            requested.query_param()
        );
        let raw = self
            .post_octet_stream(&path_and_query, Bytes::copy_from_slice(image_bytes))
            .await?;
        let parsed: DetectFacesWireResponse = serde_json::from_slice(&raw).map_err(|err| {
            UnreadableImage::NotAnalysed(format!(
                "Vision provider returned malformed DetectFaces JSON: {err}"
            ))
        })?;
        Ok(parsed
            .face_details
            .into_iter()
            .map(|face| face.into_smithy(&requested))
            .collect())
    }

    /// Calls `POST /v1/rekognition/compare-faces` using length-prefixed binary framing
    /// `[4-byte big-endian u32 source_len][source_bytes][target_bytes]`.
    pub async fn compare_faces(
        &self,
        source_bytes: &[u8],
        target_bytes: &[u8],
        similarity_threshold: f32,
        quality_filter: Option<&str>,
    ) -> Result<CompareFacesProviderOutput, UnreadableImage> {
        let frame = encode_compare_frame(source_bytes, target_bytes)?;
        let filter_token = sanitize_quality_filter(quality_filter);
        let path_and_query = format!(
            "/v1/rekognition/compare-faces?similarity_threshold={similarity_threshold}&quality_filter={filter_token}"
        );
        let raw = self.post_octet_stream(&path_and_query, frame).await?;
        let parsed: CompareFacesWireResponse = serde_json::from_slice(&raw).map_err(|err| {
            UnreadableImage::NotAnalysed(format!(
                "Vision provider returned malformed CompareFaces JSON: {err}"
            ))
        })?;
        let mut matches: Vec<CompareFacesMatch> = parsed
            .face_matches
            .into_iter()
            .map(|m| CompareFacesMatch {
                similarity: Some(m.similarity),
                face: Some(m.face.into_smithy()),
            })
            .collect();
        matches.sort_by(|left, right| {
            right
                .similarity
                .unwrap_or_default()
                .total_cmp(&left.similarity.unwrap_or_default())
        });
        let unmatched: Vec<ComparedFace> = parsed
            .unmatched_faces
            .into_iter()
            .map(ComparedFaceWire::into_smithy)
            .collect();
        Ok(CompareFacesProviderOutput {
            source_image_face: ComparedSourceImageFace {
                bounding_box: Some(parsed.source_image_face.bounding_box.into_smithy()),
                confidence: Some(parsed.source_image_face.confidence),
            },
            face_matches: matches,
            unmatched_faces: unmatched,
        })
    }

    /// Calls `POST /v1/rekognition/detect-labels` and returns labels + image properties.
    pub async fn detect_labels(
        &self,
        image_bytes: &[u8],
        requested: Requested,
    ) -> Result<(ImageAnalysis, Option<String>), UnreadableImage> {
        validate_image_ceiling(image_bytes)?;
        let features_param = match (requested.general_labels, requested.image_properties) {
            (true, true) => "GENERAL_LABELS,IMAGE_PROPERTIES",
            (false, true) => "IMAGE_PROPERTIES",
            _ => "GENERAL_LABELS",
        };
        let path_and_query = format!(
            "/v1/rekognition/detect-labels?features={features_param}&max_dominant_colors={}",
            requested.max_dominant_colors
        );
        let raw = self
            .post_octet_stream(&path_and_query, Bytes::copy_from_slice(image_bytes))
            .await?;
        let parsed: DetectLabelsWireResponse = serde_json::from_slice(&raw).map_err(|err| {
            UnreadableImage::NotAnalysed(format!(
                "Vision provider returned malformed DetectLabels JSON: {err}"
            ))
        })?;

        let labels = parsed
            .labels
            .into_iter()
            .map(LabelWire::into_smithy)
            .collect();

        let image_properties = if requested.image_properties {
            // Compute foreground/background Otsu split from local pixels when decodable so
            // Foreground/Background stay populated alongside provider Quality/DominantColors.
            let local_props = label_detection::analysed_image(
                image_bytes,
                Requested {
                    general_labels: false,
                    image_properties: true,
                    max_dominant_colors: requested.max_dominant_colors,
                },
            )
            .ok()
            .and_then(|a| a.image_properties);

            match (parsed.image_properties, local_props) {
                (Some(wire_props), Some(local)) => {
                    let mut mapped = wire_props.into_smithy();
                    mapped.foreground = local.foreground;
                    mapped.background = local.background;
                    Some(mapped)
                }
                (Some(wire_props), None) => Some(wire_props.into_smithy()),
                (None, local) => local,
            }
        } else {
            None
        };

        let model_version = if requested.general_labels {
            Some(
                parsed
                    .label_model_version
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| label_detection::LABEL_MODEL_VERSION.to_string()),
            )
        } else {
            None
        };

        Ok((
            ImageAnalysis {
                labels,
                image_properties,
            },
            model_version,
        ))
    }

    async fn post_octet_stream(
        &self,
        path_and_query: &str,
        payload: Bytes,
    ) -> Result<Vec<u8>, UnreadableImage> {
        tokio::time::timeout(
            VISION_REQUEST_TIMEOUT,
            self.post_octet_stream_inner(path_and_query, payload),
        )
        .await
        .map_err(|_| {
            UnreadableImage::NotAnalysed("Vision provider request timed out".to_string())
        })?
    }

    async fn post_octet_stream_inner(
        &self,
        path_and_query: &str,
        payload: Bytes,
    ) -> Result<Vec<u8>, UnreadableImage> {
        let (status, body) = match self {
            Self::Unix(sock_path) => {
                let stream = UnixStream::connect(sock_path).await.map_err(|err| {
                    UnreadableImage::NotAnalysed(format!(
                        "Vision provider Unix socket unreachable: {err}"
                    ))
                })?;
                send_http1_request(stream, "localhost", path_and_query, payload).await?
            }
            Self::Http {
                authority,
                connect_addr,
                base_path,
            } => {
                let stream = TcpStream::connect(connect_addr).await.map_err(|err| {
                    UnreadableImage::NotAnalysed(format!(
                        "Vision provider TCP endpoint unreachable: {err}"
                    ))
                })?;
                let full_uri = if base_path.is_empty() {
                    path_and_query.to_string()
                } else {
                    format!("{base_path}{path_and_query}")
                };
                send_http1_request(stream, authority, &full_uri, payload).await?
            }
        };

        if status.is_success() {
            return Ok(body);
        }
        Err(map_provider_error(status, &body))
    }
}

/// Resolves the configured vision endpoint for `engine`: checks the engine's per-instance
/// `vision_endpoint` override first, then `EMULATOR_VISION_ENDPOINT`, returning `Ok(None)`
/// when neither is set.
pub fn configured_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Result<Option<VisionEndpoint>, UnreadableImage> {
    if let Some(state) = super::service_state(engine) {
        let override_ep = state.lock().vision_endpoint.clone();
        if let Some(raw) = override_ep.filter(|s| !s.trim().is_empty()) {
            return VisionEndpoint::parse(&raw).map(Some);
        }
    }
    match std::env::var(ENV_VISION_ENDPOINT) {
        Ok(raw) if !raw.trim().is_empty() => VisionEndpoint::parse(&raw).map(Some),
        _ => Ok(None),
    }
}

/// Packed result of `CompareFaces` returned by [`VisionEndpoint::compare_faces`].
#[derive(Debug)]
pub struct CompareFacesProviderOutput {
    pub source_image_face: ComparedSourceImageFace,
    pub face_matches: Vec<CompareFacesMatch>,
    pub unmatched_faces: Vec<ComparedFace>,
}

/// Encodes `[4-byte big-endian u32 source.len()][source][target]` enforcing 15 MiB per image.
pub fn encode_compare_frame(source: &[u8], target: &[u8]) -> Result<Bytes, UnreadableImage> {
    if source.is_empty() || target.is_empty() {
        return Err(UnreadableImage::invalid_format());
    }
    validate_image_ceiling(source)?;
    validate_image_ceiling(target)?;
    let src_len = u32::try_from(source.len()).map_err(|_| {
        UnreadableImage::TooLarge(super::image_source::IMAGE_TOO_LARGE_MESSAGE.to_string())
    })?;
    let mut buf = Vec::with_capacity(4 + source.len() + target.len());
    buf.extend_from_slice(&src_len.to_be_bytes());
    buf.extend_from_slice(source);
    buf.extend_from_slice(target);
    Ok(Bytes::from(buf))
}

fn validate_image_ceiling(image_bytes: &[u8]) -> Result<(), UnreadableImage> {
    if image_bytes.is_empty() {
        return Err(UnreadableImage::invalid_format());
    }
    if image_bytes.len() > MAX_S3_IMAGE_BYTES {
        return Err(UnreadableImage::TooLarge(
            super::image_source::IMAGE_TOO_LARGE_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

fn has_explicit_port(authority: &str) -> bool {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest
            .split_once(']')
            .is_some_and(|(_, after)| after.starts_with(':'));
    }
    authority.contains(':')
}

fn sanitize_quality_filter(raw: Option<&str>) -> &'static str {
    match raw.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("AUTO") => "AUTO",
        Some(v) if v.eq_ignore_ascii_case("LOW") => "LOW",
        Some(v) if v.eq_ignore_ascii_case("MEDIUM") => "MEDIUM",
        Some(v) if v.eq_ignore_ascii_case("HIGH") => "HIGH",
        _ => "NONE",
    }
}

async fn send_http1_request<S>(
    stream: S,
    host_header: &str,
    uri_path_and_query: &str,
    payload: Bytes,
) -> Result<(::http::StatusCode, Vec<u8>), UnreadableImage>
where
    S: ::tokio::io::AsyncRead + ::tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) = http1::handshake(TokioIo::new(stream)).await.map_err(|err| {
        UnreadableImage::NotAnalysed(format!("Vision provider HTTP handshake failed: {err}"))
    })?;
    ::tokio::spawn(async move {
        let _ = conn.await;
    });

    let content_len = payload.len();
    let req = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(uri_path_and_query)
        .header(::http::header::HOST, host_header)
        .header(::http::header::CONTENT_TYPE, "application/octet-stream")
        .header(::http::header::CONTENT_LENGTH, content_len)
        .body(Full::new(payload))
        .map_err(|err| {
            UnreadableImage::NotAnalysed(format!("Failed to build vision provider request: {err}"))
        })?;

    let resp = sender.send_request(req).await.map_err(|err| {
        UnreadableImage::NotAnalysed(format!("Vision provider HTTP request failed: {err}"))
    })?;
    let status = resp.status();
    let mut body_stream = resp.into_body();
    let mut collected = Vec::new();
    while let Some(frame_res) = body_stream.frame().await {
        let frame = frame_res.map_err(|err| {
            UnreadableImage::NotAnalysed(format!(
                "Failed reading vision provider response body: {err}"
            ))
        })?;
        if let Ok(chunk) = frame.into_data() {
            if collected.len().saturating_add(chunk.len()) > MAX_VISION_RESPONSE_BYTES {
                return Err(UnreadableImage::NotAnalysed(
                    "Vision provider response exceeded 4 MiB safety limit".to_string(),
                ));
            }
            collected.extend_from_slice(&chunk);
        }
    }
    Ok((status, collected))
}

#[derive(Debug, Deserialize)]
struct ProviderErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

fn map_provider_error(status: ::http::StatusCode, body: &[u8]) -> UnreadableImage {
    let parsed = serde_json::from_slice::<ProviderErrorBody>(body).ok();
    let code = parsed.as_ref().map_or("", |p| p.code.as_str());
    let msg = parsed
        .as_ref()
        .map(|p| p.message.trim())
        .filter(|m| !m.is_empty())
        .map(ToString::to_string);

    match code {
        "InvalidImageFormatException" => UnreadableImage::InvalidFormat(
            msg.unwrap_or_else(|| super::image_source::INVALID_IMAGE_FORMAT_MESSAGE.to_string()),
        ),
        "ImageTooLargeException" => UnreadableImage::TooLarge(
            msg.unwrap_or_else(|| super::image_source::IMAGE_TOO_LARGE_MESSAGE.to_string()),
        ),
        "InvalidParameterException" => UnreadableImage::InvalidParameter(
            msg.unwrap_or_else(|| "Request has invalid parameters".to_string()),
        ),
        _ if status == ::http::StatusCode::PAYLOAD_TOO_LARGE => UnreadableImage::TooLarge(
            msg.unwrap_or_else(|| super::image_source::IMAGE_TOO_LARGE_MESSAGE.to_string()),
        ),
        _ => UnreadableImage::NotAnalysed(
            msg.unwrap_or_else(|| format!("Vision provider failed with HTTP {}", status.as_u16())),
        ),
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
struct RequestedAttributes {
    all: bool,
    age_range: bool,
    beard: bool,
    emotions: bool,
    eye_direction: bool,
    eyeglasses: bool,
    eyes_open: bool,
    face_occluded: bool,
    gender: bool,
    mouth_open: bool,
    mustache: bool,
    smile: bool,
    sunglasses: bool,
}

impl RequestedAttributes {
    fn from_slice(raw: Option<&[String]>) -> Self {
        let mut out = Self::default();
        for item in raw.unwrap_or_default() {
            for tok in item.split(',') {
                match tok.trim() {
                    "" | "DEFAULT" => {}
                    "ALL" => out.all = true,
                    "AGE_RANGE" => out.age_range = true,
                    "BEARD" => out.beard = true,
                    "EMOTIONS" => out.emotions = true,
                    "EYE_DIRECTION" => out.eye_direction = true,
                    "EYEGLASSES" => out.eyeglasses = true,
                    "EYES_OPEN" => out.eyes_open = true,
                    "FACE_OCCLUDED" => out.face_occluded = true,
                    "GENDER" => out.gender = true,
                    "MOUTH_OPEN" => out.mouth_open = true,
                    "MUSTACHE" => out.mustache = true,
                    "SMILE" => out.smile = true,
                    "SUNGLASSES" => out.sunglasses = true,
                    _ => {}
                }
            }
        }
        if out.all {
            out.age_range = true;
            out.beard = true;
            out.emotions = true;
            out.eye_direction = true;
            out.eyeglasses = true;
            out.eyes_open = true;
            out.face_occluded = true;
            out.gender = true;
            out.mouth_open = true;
            out.mustache = true;
            out.smile = true;
            out.sunglasses = true;
        }
        out
    }

    fn query_param(self) -> String {
        if self.all {
            return "ALL".to_string();
        }
        let mut names = Vec::new();
        if self.age_range {
            names.push("AGE_RANGE");
        }
        if self.beard {
            names.push("BEARD");
        }
        if self.emotions {
            names.push("EMOTIONS");
        }
        if self.eye_direction {
            names.push("EYE_DIRECTION");
        }
        if self.eyeglasses {
            names.push("EYEGLASSES");
        }
        if self.eyes_open {
            names.push("EYES_OPEN");
        }
        if self.face_occluded {
            names.push("FACE_OCCLUDED");
        }
        if self.gender {
            names.push("GENDER");
        }
        if self.mouth_open {
            names.push("MOUTH_OPEN");
        }
        if self.mustache {
            names.push("MUSTACHE");
        }
        if self.smile {
            names.push("SMILE");
        }
        if self.sunglasses {
            names.push("SUNGLASSES");
        }
        if names.is_empty() {
            "DEFAULT".to_string()
        } else {
            names.join(",")
        }
    }
}

#[derive(Debug, Deserialize)]
struct BoundingBoxWire {
    width: f32,
    height: f32,
    left: f32,
    top: f32,
}

impl BoundingBoxWire {
    fn into_smithy(self) -> BoundingBox {
        BoundingBox {
            width: Some(self.width),
            height: Some(self.height),
            left: Some(self.left),
            top: Some(self.top),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LandmarkWire {
    #[serde(rename = "type")]
    kind: String,
    x: f32,
    y: f32,
}

impl LandmarkWire {
    fn into_smithy(self) -> Landmark {
        Landmark {
            r#type: Some(self.kind),
            x: Some(self.x),
            y: Some(self.y),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PoseWire {
    roll: f32,
    yaw: f32,
    pitch: f32,
}

impl PoseWire {
    fn into_smithy(self) -> Pose {
        Pose {
            roll: Some(self.roll),
            yaw: Some(self.yaw),
            pitch: Some(self.pitch),
        }
    }
}

#[derive(Debug, Deserialize)]
struct QualityWire {
    brightness: f32,
    sharpness: f32,
}

impl QualityWire {
    fn into_smithy(self) -> ImageQuality {
        ImageQuality {
            brightness: Some(self.brightness),
            sharpness: Some(self.sharpness),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgeRangeWire {
    low: i32,
    high: i32,
}

#[derive(Debug, Deserialize)]
struct BoolAttrWire {
    value: bool,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct GenderWire {
    value: String,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct EmotionWire {
    #[serde(rename = "type")]
    kind: String,
    confidence: f32,
}

impl EmotionWire {
    fn into_smithy(self) -> Emotion {
        Emotion {
            r#type: Some(self.kind),
            confidence: Some(self.confidence),
        }
    }
}

#[derive(Debug, Deserialize)]
struct EyeDirectionWire {
    yaw: f32,
    pitch: f32,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct FaceDetailWire {
    bounding_box: BoundingBoxWire,
    confidence: f32,
    #[serde(default)]
    landmarks: Vec<LandmarkWire>,
    pose: PoseWire,
    quality: QualityWire,
    #[serde(default)]
    age_range: Option<AgeRangeWire>,
    #[serde(default)]
    smile: Option<BoolAttrWire>,
    #[serde(default)]
    eyeglasses: Option<BoolAttrWire>,
    #[serde(default)]
    sunglasses: Option<BoolAttrWire>,
    #[serde(default)]
    gender: Option<GenderWire>,
    #[serde(default)]
    beard: Option<BoolAttrWire>,
    #[serde(default)]
    mustache: Option<BoolAttrWire>,
    #[serde(default)]
    eyes_open: Option<BoolAttrWire>,
    #[serde(default)]
    mouth_open: Option<BoolAttrWire>,
    #[serde(default)]
    emotions: Option<Vec<EmotionWire>>,
    #[serde(default)]
    face_occluded: Option<BoolAttrWire>,
    #[serde(default)]
    eye_direction: Option<EyeDirectionWire>,
}

impl FaceDetailWire {
    fn into_smithy(self, attrs: &RequestedAttributes) -> FaceDetail {
        FaceDetail {
            bounding_box: Some(self.bounding_box.into_smithy()),
            confidence: Some(self.confidence),
            landmarks: Some(
                self.landmarks
                    .into_iter()
                    .map(LandmarkWire::into_smithy)
                    .collect(),
            ),
            pose: Some(self.pose.into_smithy()),
            quality: Some(self.quality.into_smithy()),
            age_range: attrs
                .age_range
                .then_some(self.age_range)
                .flatten()
                .map(|ar| AgeRange {
                    low: Some(ar.low),
                    high: Some(ar.high),
                }),
            smile: attrs.smile.then_some(self.smile).flatten().map(|s| Smile {
                value: s.value,
                confidence: Some(s.confidence),
            }),
            eyeglasses: attrs
                .eyeglasses
                .then_some(self.eyeglasses)
                .flatten()
                .map(|g| Eyeglasses {
                    value: g.value,
                    confidence: Some(g.confidence),
                }),
            sunglasses: attrs
                .sunglasses
                .then_some(self.sunglasses)
                .flatten()
                .map(|s| Sunglasses {
                    value: s.value,
                    confidence: Some(s.confidence),
                }),
            gender: attrs
                .gender
                .then_some(self.gender)
                .flatten()
                .map(|g| Gender {
                    value: Some(g.value),
                    confidence: Some(g.confidence),
                }),
            beard: attrs.beard.then_some(self.beard).flatten().map(|b| Beard {
                value: b.value,
                confidence: Some(b.confidence),
            }),
            mustache: attrs
                .mustache
                .then_some(self.mustache)
                .flatten()
                .map(|m| Mustache {
                    value: m.value,
                    confidence: Some(m.confidence),
                }),
            eyes_open: attrs
                .eyes_open
                .then_some(self.eyes_open)
                .flatten()
                .map(|e| EyeOpen {
                    value: e.value,
                    confidence: Some(e.confidence),
                }),
            mouth_open: attrs
                .mouth_open
                .then_some(self.mouth_open)
                .flatten()
                .map(|m| MouthOpen {
                    value: m.value,
                    confidence: Some(m.confidence),
                }),
            emotions: attrs
                .emotions
                .then_some(self.emotions)
                .flatten()
                .map(|ems| ems.into_iter().map(EmotionWire::into_smithy).collect()),
            face_occluded: attrs
                .face_occluded
                .then_some(self.face_occluded)
                .flatten()
                .map(|o| FaceOccluded {
                    value: o.value,
                    confidence: Some(o.confidence),
                }),
            eye_direction: attrs
                .eye_direction
                .then_some(self.eye_direction)
                .flatten()
                .map(|ed| EyeDirection {
                    yaw: Some(ed.yaw),
                    pitch: Some(ed.pitch),
                    confidence: Some(ed.confidence),
                }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetectFacesWireResponse {
    #[serde(default)]
    face_details: Vec<FaceDetailWire>,
}

#[derive(Debug, Deserialize)]
struct ComparedSourceImageFaceWire {
    bounding_box: BoundingBoxWire,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct ComparedFaceWire {
    bounding_box: BoundingBoxWire,
    confidence: f32,
    #[serde(default)]
    landmarks: Option<Vec<LandmarkWire>>,
    #[serde(default)]
    pose: Option<PoseWire>,
    #[serde(default)]
    quality: Option<QualityWire>,
    #[serde(default)]
    smile: Option<BoolAttrWire>,
    #[serde(default)]
    emotions: Option<Vec<EmotionWire>>,
}

impl ComparedFaceWire {
    fn into_smithy(self) -> ComparedFace {
        ComparedFace {
            bounding_box: Some(self.bounding_box.into_smithy()),
            confidence: Some(self.confidence),
            landmarks: self
                .landmarks
                .map(|lms| lms.into_iter().map(LandmarkWire::into_smithy).collect()),
            pose: self.pose.map(PoseWire::into_smithy),
            quality: self.quality.map(QualityWire::into_smithy),
            smile: self.smile.map(|s| Smile {
                value: s.value,
                confidence: Some(s.confidence),
            }),
            emotions: self
                .emotions
                .map(|ems| ems.into_iter().map(EmotionWire::into_smithy).collect()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompareFacesMatchWire {
    similarity: f32,
    face: ComparedFaceWire,
}

#[derive(Debug, Deserialize)]
struct CompareFacesWireResponse {
    source_image_face: ComparedSourceImageFaceWire,
    #[serde(default)]
    face_matches: Vec<CompareFacesMatchWire>,
    #[serde(default)]
    unmatched_faces: Vec<ComparedFaceWire>,
}

#[derive(Debug, Deserialize)]
struct LabelParentWire {
    name: String,
}

#[derive(Debug, Deserialize)]
struct LabelInstanceWire {
    bounding_box: BoundingBoxWire,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct LabelWire {
    name: String,
    confidence: f32,
    #[serde(default)]
    instances: Vec<LabelInstanceWire>,
    #[serde(default)]
    parents: Vec<LabelParentWire>,
}

impl LabelWire {
    fn into_smithy(self) -> Label {
        Label {
            name: Some(self.name),
            confidence: Some(self.confidence),
            instances: Some(
                self.instances
                    .into_iter()
                    .map(|inst| Instance {
                        bounding_box: Some(inst.bounding_box.into_smithy()),
                        confidence: Some(inst.confidence),
                        dominant_colors: None,
                    })
                    .collect(),
            ),
            parents: Some(
                self.parents
                    .into_iter()
                    .map(|p| Parent { name: Some(p.name) })
                    .collect(),
            ),
            aliases: None,
            categories: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetectLabelsQualityWire {
    brightness: f32,
    sharpness: f32,
    contrast: f32,
}

#[derive(Debug, Deserialize)]
struct DominantColorWire {
    red: i32,
    green: i32,
    blue: i32,
    hex_code: String,
    simplified_color: String,
    pixel_percent: f32,
}

#[derive(Debug, Deserialize)]
struct DetectLabelsImagePropertiesWire {
    quality: DetectLabelsQualityWire,
    #[serde(default)]
    dominant_colors: Vec<DominantColorWire>,
}

impl DetectLabelsImagePropertiesWire {
    fn into_smithy(self) -> DetectLabelsImageProperties {
        DetectLabelsImageProperties {
            quality: Some(DetectLabelsImageQuality {
                brightness: Some(self.quality.brightness),
                sharpness: Some(self.quality.sharpness),
                contrast: Some(self.quality.contrast),
            }),
            dominant_colors: Some(
                self.dominant_colors
                    .into_iter()
                    .map(|c| DominantColor {
                        red: Some(c.red),
                        green: Some(c.green),
                        blue: Some(c.blue),
                        hex_code: Some(c.hex_code),
                        simplified_color: Some(c.simplified_color),
                        pixel_percent: Some(c.pixel_percent),
                        css_color: None,
                    })
                    .collect(),
            ),
            foreground: None,
            background: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DetectLabelsWireResponse {
    #[serde(default)]
    labels: Vec<LabelWire>,
    #[serde(default)]
    label_model_version: Option<String>,
    #[serde(default)]
    image_properties: Option<DetectLabelsImagePropertiesWire>,
}
