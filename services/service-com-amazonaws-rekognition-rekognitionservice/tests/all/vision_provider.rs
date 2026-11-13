// SPDX-License-Identifier: CC0-1.0

//! End-to-end verification of the external vision provider capability adapter
//! (`state::vision_provider`) over both Unix domain sockets (`unix:///.../vision.sock`)
//! and loopback TCP (`http://127.0.0.1:<port>`).

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use ::bytes::Bytes;
use ::http_body_util::{BodyExt, Full};
use ::hyper::server::conn::http1 as server_http1;
use ::hyper::service::service_fn;
use ::hyper_util::rt::TokioIo;
use ::image::{GrayImage, ImageFormat, Luma};
use ::serde_json::{Value, json};
use ::service_com_amazonaws_rekognition_rekognitionservice::{
    ComAmazonawsRekognitionRekognitionserviceState, EmulatedService,
    state::vision_provider::VisionEndpoint,
};
use ::smithy_server_runtime::MultiServiceRouter;
use ::tokio::net::{TcpListener, UnixListener};

use crate::face_collections::{bytes_image, call};
use crate::face_detection::{BUCKET, DrawnFace, encode, s3};

fn draw_portrait(bg: u8, skin: u8, feature_tone: u8, has_face: bool) -> GrayImage {
    let mut img = GrayImage::from_pixel(128, 128, Luma([bg]));
    if !has_face {
        return img;
    }
    let face = DrawnFace::new(64.0, 56.0, 26.0);
    for y in 20..108_u32 {
        for x in 32..96_u32 {
            let dx = (x as f32 - 64.0) / 28.0;
            let dy = (y as f32 - 62.0) / 38.0;
            if dx * dx + dy * dy <= 1.0 {
                img.put_pixel(x, y, Luma([skin]));
            }
        }
    }
    // Crisp eyes, eyeglass bridge, nose, and smiling mouth so pixel brightness and Laplacian
    // variance are genuinely measured from the submitted image bytes.
    let _ = face;
    for x in 45..55_u32 {
        for y in 52..57_u32 {
            img.put_pixel(x, y, Luma([feature_tone]));
        }
    }
    for x in 73..83_u32 {
        for y in 52..57_u32 {
            img.put_pixel(x, y, Luma([feature_tone]));
        }
    }
    for x in 55..73_u32 {
        img.put_pixel(x, 54, Luma([feature_tone]));
    }
    for y in 60..70_u32 {
        img.put_pixel(64, y, Luma([feature_tone.saturating_add(20)]));
    }
    for x in 50..78_u32 {
        for y in 80..85_u32 {
            img.put_pixel(x, y, Luma([feature_tone]));
        }
    }
    img
}

fn harness_with_vision_endpoint(endpoint: &str) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    engine
        .state::<ComAmazonawsRekognitionRekognitionserviceState>()
        .lock()
        .vision_endpoint = Some(endpoint.to_string());

    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// Decodes PNG/JPEG bytes inside the test vision daemon and computes real luminance brightness,
/// 2D Laplacian variance sharpness, and 8x8 spatial feature vector for cosine similarity.
fn analyze_pixels(raw: &[u8]) -> Result<(bool, f32, f32, [f64; 64]), (u16, Value)> {
    let decoded = ::image::load_from_memory(raw).map_err(|_| {
        (
            400,
            json!({
                "code": "InvalidImageFormatException",
                "message": "Request has invalid image format",
            }),
        )
    })?;
    let gray = decoded.to_luma8();
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    if w == 0 || h == 0 {
        return Err((
            400,
            json!({
                "code": "InvalidImageFormatException",
                "message": "Request has invalid image format",
            }),
        ));
    }

    let mut sum = 0.0_f64;
    for p in gray.pixels() {
        sum += f64::from(p.0[0]);
    }
    let brightness = ((sum / (w * h) as f64) / 255.0 * 100.0) as f32;

    let mut lap_sum = 0.0_f64;
    let mut lap_sq = 0.0_f64;
    let mut lap_n = 0_usize;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let c = i32::from(gray.get_pixel(x as u32, y as u32).0[0]);
            let l = i32::from(gray.get_pixel((x - 1) as u32, y as u32).0[0]);
            let r = i32::from(gray.get_pixel((x + 1) as u32, y as u32).0[0]);
            let u = i32::from(gray.get_pixel(x as u32, (y - 1) as u32).0[0]);
            let d = i32::from(gray.get_pixel(x as u32, (y + 1) as u32).0[0]);
            let lap = f64::from(l + r + u + d - 4 * c);
            lap_sum += lap;
            lap_sq += lap * lap;
            lap_n += 1;
        }
    }
    let lap_var = if lap_n > 0 {
        let m = lap_sum / lap_n as f64;
        (lap_sq / lap_n as f64 - m * m).max(0.0)
    } else {
        0.0
    };
    let sharpness = (100.0 * lap_var / (lap_var + 625.0)) as f32;

    let center = f64::from(gray.get_pixel((w / 2) as u32, (h / 2) as u32).0[0]);
    let corner = f64::from(gray.get_pixel(0, 0).0[0]);
    let has_face = (center - corner).abs() >= 10.0 && sharpness > 0.5;

    let mut feat = [0.0_f64; 64];
    for gy in 0..8_usize {
        for gx in 0..8_usize {
            let x0 = gx * w / 8;
            let x1 = ((gx + 1) * w / 8).max(x0 + 1);
            let y0 = gy * h / 8;
            let y1 = ((gy + 1) * h / 8).max(y0 + 1);
            let mut cell = 0.0_f64;
            let mut cnt = 0_usize;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    cell += f64::from(gray.get_pixel(x as u32, y as u32).0[0]) / 255.0;
                    cnt += 1;
                }
            }
            let mean = cell / cnt.max(1) as f64;
            feat[gy * 8 + gx] = ((gy + 1) as f64).mul_add(0.03, mean)
                * (((gx + 1) as f64 * (mean + 0.2)).cos());
        }
    }

    Ok((has_face, brightness, sharpness, feat))
}

fn cosine_pct(a: &[f64; 64], b: &[f64; 64]) -> f32 {
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for i in 0..64 {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    ((dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0) * 100.0) as f32
}

fn full_face_json(brightness: f32, sharpness: f32) -> Value {
    json!({
        "bounding_box": { "left": 0.25, "top": 0.16, "width": 0.50, "height": 0.68 },
        "confidence": 98.7,
        "landmarks": [
            { "type": "eyeLeft", "x": 0.39, "y": 0.42 },
            { "type": "eyeRight", "x": 0.61, "y": 0.43 },
            { "type": "nose", "x": 0.50, "y": 0.52 },
            { "type": "mouthLeft", "x": 0.41, "y": 0.64 },
            { "type": "mouthRight", "x": 0.59, "y": 0.65 }
        ],
        "pose": { "roll": 2.4, "yaw": -5.8, "pitch": 3.1 },
        "quality": { "brightness": brightness, "sharpness": sharpness },
        "age_range": { "low": 24, "high": 32 },
        "smile": { "value": true, "confidence": 95.4 },
        "eyeglasses": { "value": true, "confidence": 92.8 },
        "sunglasses": { "value": false, "confidence": 98.6 },
        "gender": { "value": "Female", "confidence": 96.2 },
        "beard": { "value": false, "confidence": 99.1 },
        "mustache": { "value": false, "confidence": 99.3 },
        "eyes_open": { "value": true, "confidence": 97.9 },
        "mouth_open": { "value": false, "confidence": 91.0 },
        "emotions": [
            { "type": "HAPPY", "confidence": 94.2 },
            { "type": "CALM", "confidence": 5.8 }
        ],
        "face_occluded": { "value": false, "confidence": 97.5 },
        "eye_direction": { "yaw": -2.1, "pitch": 1.3, "confidence": 93.0 }
    })
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

async fn handle_mock_vision(
    req: ::http::Request<::hyper::body::Incoming>,
) -> Result<::http::Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(ToString::to_string);
    let body = req
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();

    let (status, payload) = match path.as_str() {
        "/v1/rekognition/detect-faces" => match analyze_pixels(&body) {
            Ok((false, _, _, _)) => (200, json!({ "face_details": [] })),
            Ok((true, b, s, _)) => (200, json!({ "face_details": [full_face_json(b, s)] })),
            Err(err) => err,
        },
        "/v1/rekognition/compare-faces" => {
            if body.len() < 4 {
                (
                    400,
                    json!({ "code": "InvalidParameterException", "message": "Truncated frame" }),
                )
            } else {
                let src_len =
                    u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                if body.len() <= 4 + src_len {
                    (
                        400,
                        json!({ "code": "InvalidParameterException", "message": "Short frame" }),
                    )
                } else {
                    let src = &body[4..4 + src_len];
                    let tgt = &body[4 + src_len..];
                    let thresh: f32 = query_value(query.as_deref(), "similarity_threshold")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(80.0);
                    match (analyze_pixels(src), analyze_pixels(tgt)) {
                        (Ok((true, _, _, fa)), Ok((true, tb, ts, fb))) => {
                            let sim = cosine_pct(&fa, &fb);
                            let face = json!({
                                "bounding_box": { "left": 0.25, "top": 0.16, "width": 0.50, "height": 0.68 },
                                "confidence": 98.7,
                                "landmarks": [
                                    { "type": "eyeLeft", "x": 0.39, "y": 0.42 },
                                    { "type": "eyeRight", "x": 0.61, "y": 0.43 }
                                ],
                                "pose": { "roll": 2.4, "yaw": -5.8, "pitch": 3.1 },
                                "quality": { "brightness": tb, "sharpness": ts },
                                "smile": { "value": true, "confidence": 95.4 },
                                "emotions": [{ "type": "HAPPY", "confidence": 94.2 }]
                            });
                            let (matches, unmatched) = if sim >= thresh {
                                (vec![json!({ "similarity": sim, "face": face })], vec![])
                            } else {
                                (vec![], vec![face])
                            };
                            (
                                200,
                                json!({
                                    "source_image_face": {
                                        "bounding_box": { "left": 0.25, "top": 0.16, "width": 0.50, "height": 0.68 },
                                        "confidence": 98.7
                                    },
                                    "face_matches": matches,
                                    "unmatched_faces": unmatched
                                }),
                            )
                        }
                        (Ok(_), Ok(_)) => (
                            400,
                            json!({
                                "code": "InvalidParameterException",
                                "message": "Request has invalid parameters"
                            }),
                        ),
                        (Err(e), _) | (_, Err(e)) => e,
                    }
                }
            }
        }
        "/v1/rekognition/detect-labels" => match analyze_pixels(&body) {
            Ok((has_face, b, s, _)) => {
                let features =
                    query_value(query.as_deref(), "features").unwrap_or_default();
                let want_general = features.is_empty() || features.contains("GENERAL_LABELS");
                let want_props = features.contains("IMAGE_PROPERTIES");
                let labels = if want_general && has_face {
                    vec![
                        json!({
                            "name": "Face",
                            "confidence": 98.7,
                            "instances": [{
                                "bounding_box": { "left": 0.25, "top": 0.16, "width": 0.50, "height": 0.68 },
                                "confidence": 98.7
                            }],
                            "parents": [{ "name": "Person" }]
                        }),
                        json!({
                            "name": "Person",
                            "confidence": 98.7,
                            "parents": []
                        }),
                        json!({
                            "name": "Glasses",
                            "confidence": 92.8,
                            "parents": [{ "name": "Accessories" }]
                        }),
                        json!({
                            "name": "Smile",
                            "confidence": 74.0,
                            "parents": [{ "name": "Face" }, { "name": "Person" }]
                        }),
                    ]
                } else {
                    vec![]
                };
                let props = want_props.then(|| {
                    json!({
                        "quality": { "brightness": b, "sharpness": s, "contrast": 42.5 },
                        "dominant_colors": [{
                            "red": 210,
                            "green": 210,
                            "blue": 210,
                            "hex_code": "#d2d2d2",
                            "simplified_color": "white",
                            "pixel_percent": 55.0
                        }]
                    })
                });
                (
                    200,
                    json!({
                        "labels": labels,
                        "label_model_version": want_general.then_some("3.0"),
                        "image_properties": props
                    }),
                )
            }
            Err(err) => err,
        },
        _ => (
            404,
            json!({ "code": "InternalServerError", "message": "Unknown route" }),
        ),
    };

    let bytes = Bytes::from(serde_json::to_vec(&payload).expect("valid json"));
    Ok(::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(bytes))
        .expect("response builds"))
}

fn spawn_unix_mock_server() -> (PathBuf, ::tokio::task::JoinHandle<()>) {
    let unique = format!(
        "rekog-vision-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let sock_path = std::env::temp_dir().join(unique);
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind unix socket");
    let handle = ::tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            ::tokio::spawn(async move {
                let _ = server_http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service_fn(handle_mock_vision))
                    .await;
            });
        }
    });
    (sock_path, handle)
}

async fn spawn_tcp_mock_server() -> (String, ::tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback tcp");
    let addr = listener.local_addr().expect("local addr");
    let handle = ::tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            ::tokio::spawn(async move {
                let _ = server_http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service_fn(handle_mock_vision))
                    .await;
            });
        }
    });
    (format!("http://{addr}"), handle)
}

#[test]
fn endpoint_parser_validates_unix_and_http_schemes() {
    assert_eq!(
        VisionEndpoint::parse("unix:///run/rekognition/vision.sock").expect("valid unix"),
        VisionEndpoint::Unix(PathBuf::from("/run/rekognition/vision.sock"))
    );
    assert_eq!(
        VisionEndpoint::parse("/tmp/rekognition.sock").expect("bare unix path"),
        VisionEndpoint::Unix(PathBuf::from("/tmp/rekognition.sock"))
    );
    assert_eq!(
        VisionEndpoint::parse("http://127.0.0.1:9299/prefix/").expect("valid http"),
        VisionEndpoint::Http {
            authority: "127.0.0.1:9299".to_string(),
            connect_addr: "127.0.0.1:9299".to_string(),
            base_path: "/prefix".to_string(),
        }
    );
    assert!(VisionEndpoint::parse("").is_err());
    assert!(VisionEndpoint::parse("unix://relative/path.sock").is_err());
    assert!(VisionEndpoint::parse("https://example.com").is_err());
}

#[tokio::test]
async fn detect_faces_over_unix_socket_populates_all_seventeen_attributes_and_honors_default() {
    let (sock_path, server_task) = spawn_unix_mock_server();
    let endpoint_uri = format!("unix://{}", sock_path.display());
    let router = harness_with_vision_endpoint(&endpoint_uri);

    let portrait_bytes = encode(&draw_portrait(25, 215, 20, true), ImageFormat::Png);

    // Stage the same portrait in S3 to prove S3Object resolution sends bounded bytes over Unix socket.
    assert_eq!(s3(&router, &format!("/{BUCKET}"), &[]).await, 200);
    assert_eq!(
        s3(&router, &format!("/{BUCKET}/selfie.png"), &portrait_bytes).await,
        200
    );

    // 1. Attributes: ["ALL"] on S3Object must populate all 17 FaceDetail members!
    let reply_all = call(
        &router,
        "DetectFaces",
        json!({
            "Image": { "S3Object": { "Bucket": BUCKET, "Name": "selfie.png" } },
            "Attributes": ["ALL"]
        }),
    )
    .await;
    assert_eq!(reply_all.status, 200, "unexpected reply: {reply_all:?}");
    let faces = reply_all.body["FaceDetails"]
        .as_array()
        .expect("FaceDetails array");
    assert_eq!(faces.len(), 1);
    let f = &faces[0];

    for required_member in [
        "BoundingBox",
        "Confidence",
        "Landmarks",
        "Pose",
        "Quality",
        "AgeRange",
        "Smile",
        "Eyeglasses",
        "Sunglasses",
        "Gender",
        "Beard",
        "Mustache",
        "EyesOpen",
        "MouthOpen",
        "Emotions",
        "FaceOccluded",
        "EyeDirection",
    ] {
        assert!(
            f.get(required_member).is_some_and(|v| !v.is_null()),
            "missing or null FaceDetail member {required_member} in {f}"
        );
    }
    assert!(f["Pose"]["Yaw"].as_f64().is_some());
    assert!(f["Pose"]["Pitch"].as_f64().is_some());
    assert!(f["Pose"]["Roll"].as_f64().is_some());
    assert_eq!(f["Smile"]["Value"].as_bool(), Some(true));
    assert_eq!(f["Eyeglasses"]["Value"].as_bool(), Some(true));
    assert_eq!(f["Sunglasses"]["Value"].as_bool(), Some(false));
    assert_eq!(f["EyesOpen"]["Value"].as_bool(), Some(true));
    assert_eq!(f["MouthOpen"]["Value"].as_bool(), Some(false));
    assert_eq!(f["FaceOccluded"]["Value"].as_bool(), Some(false));
    assert!(f["Quality"]["Brightness"].as_f64().unwrap_or_default() > 10.0);
    assert!(f["Quality"]["Sharpness"].as_f64().unwrap_or_default() > 5.0);

    // 2. Attributes: ["DEFAULT"] must omit all 12 classifier attributes while retaining 3D Pose & Quality.
    let reply_def = call(
        &router,
        "DetectFaces",
        json!({
            "Image": bytes_image(&portrait_bytes),
            "Attributes": ["DEFAULT"]
        }),
    )
    .await;
    assert_eq!(reply_def.status, 200);
    let f_def = &reply_def.body["FaceDetails"][0];
    assert!(f_def["Pose"]["Yaw"].as_f64().is_some());
    assert!(f_def["Quality"]["Sharpness"].as_f64().is_some());
    for omitted in [
        "AgeRange",
        "Smile",
        "Eyeglasses",
        "Sunglasses",
        "Gender",
        "Beard",
        "Mustache",
        "EyesOpen",
        "MouthOpen",
        "Emotions",
        "FaceOccluded",
        "EyeDirection",
    ] {
        assert!(
            f_def.get(omitted).is_none(),
            "expected {omitted} absent on DEFAULT, got {f_def}"
        );
    }

    // 3. Selective Attributes: ["SMILE", "EYEGLASSES"] populates only those two beyond DEFAULT.
    let reply_sel = call(
        &router,
        "DetectFaces",
        json!({
            "Image": bytes_image(&portrait_bytes),
            "Attributes": ["SMILE", "EYEGLASSES"]
        }),
    )
    .await;
    assert_eq!(reply_sel.status, 200);
    let f_sel = &reply_sel.body["FaceDetails"][0];
    assert_eq!(f_sel["Smile"]["Value"].as_bool(), Some(true));
    assert_eq!(f_sel["Eyeglasses"]["Value"].as_bool(), Some(true));
    assert!(f_sel.get("AgeRange").is_none());
    assert!(f_sel.get("Sunglasses").is_none());

    server_task.abort();
    let _ = std::fs::remove_file(&sock_path);
}

#[tokio::test]
async fn compare_faces_and_detect_labels_over_tcp_and_honest_unreachable_error() {
    let (tcp_url, server_task) = spawn_tcp_mock_server().await;
    let router = harness_with_vision_endpoint(&tcp_url);

    let portrait_a = encode(&draw_portrait(20, 220, 15, true), ImageFormat::Jpeg);
    let portrait_b = encode(&draw_portrait(230, 80, 250, true), ImageFormat::Png);
    let blank = encode(&draw_portrait(120, 120, 120, false), ImageFormat::Png);

    // 1. CompareFaces same portrait at 90% threshold -> 1 match >= 90%, 0 unmatched.
    let same_reply = call(
        &router,
        "CompareFaces",
        json!({
            "SourceImage": bytes_image(&portrait_a),
            "TargetImage": bytes_image(&portrait_a),
            "SimilarityThreshold": 90.0
        }),
    )
    .await;
    assert_eq!(same_reply.status, 200, "unexpected: {same_reply:?}");
    let matches = same_reply.body["FaceMatches"]
        .as_array()
        .expect("FaceMatches");
    assert_eq!(matches.len(), 1);
    assert!(matches[0]["Similarity"].as_f64().unwrap_or_default() >= 99.0);
    assert_eq!(matches[0]["Face"]["Smile"]["Value"].as_bool(), Some(true));
    assert_eq!(
        same_reply.body["UnmatchedFaces"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    // 2. CompareFaces distinct portrait at 99.5% threshold -> 0 matches, 1 unmatched.
    let diff_reply = call(
        &router,
        "CompareFaces",
        json!({
            "SourceImage": bytes_image(&portrait_a),
            "TargetImage": bytes_image(&portrait_b),
            "SimilarityThreshold": 99.5
        }),
    )
    .await;
    assert_eq!(diff_reply.status, 200);
    assert_eq!(
        diff_reply.body["FaceMatches"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        diff_reply.body["UnmatchedFaces"].as_array().map(Vec::len),
        Some(1)
    );

    // 3. CompareFaces with blank source image -> 400 InvalidParameterException.
    let no_face_reply = call(
        &router,
        "CompareFaces",
        json!({
            "SourceImage": bytes_image(&blank),
            "TargetImage": bytes_image(&portrait_a),
            "SimilarityThreshold": 90.0
        }),
    )
    .await;
    assert_eq!(no_face_reply.status, 400);
    assert_eq!(no_face_reply.error_code(), "InvalidParameterException");

    // 4. DetectLabels narrows provider labels via MinConfidence + MaxLabels and includes ImageProperties.
    let labels_reply = call(
        &router,
        "DetectLabels",
        json!({
            "Image": bytes_image(&portrait_a),
            "Features": ["GENERAL_LABELS", "IMAGE_PROPERTIES"],
            "MinConfidence": 80.0,
            "MaxLabels": 3
        }),
    )
    .await;
    assert_eq!(labels_reply.status, 200, "unexpected: {labels_reply:?}");
    let labels = labels_reply.body["Labels"].as_array().expect("Labels");
    assert_eq!(labels.len(), 3, "Smile (74%) must be filtered out below 80%");
    assert!(labels.iter().any(|l| l["Name"] == "Glasses"));
    assert!(labels_reply.body["ImageProperties"]["Quality"]["Sharpness"].is_number());
    assert!(labels_reply.body["ImageProperties"]["DominantColors"].is_array());

    server_task.abort();

    // 5. Honest failure when configured Unix socket does not exist -> 500 InternalServerError.
    let dead_router =
        harness_with_vision_endpoint("unix:///tmp/nonexistent-observatory-rekognition-test.sock");
    let dead_reply = call(
        &dead_router,
        "DetectFaces",
        json!({
            "Image": bytes_image(&portrait_a),
            "Attributes": ["ALL"]
        }),
    )
    .await;
    assert_eq!(dead_reply.status, 500);
    assert_eq!(dead_reply.error_code(), "InternalServerError");
}
