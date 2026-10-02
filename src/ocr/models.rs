use prost::Message;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Instant;

/// Represents an OCR-detected text region with normalized coordinates.
/// All coordinate fields (center_x, center_y, width, height) are in the range [0.0, 1.0],
/// relative to the full game/video rendered area.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrBox {
    pub text: String,
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<ParsedLine>,
}

/// Represents an individual OCR-detected line prior to multi-line block grouping.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedLine {
    pub text: String,
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    pub paragraph_idx: usize,
}

/// State of the OCR system in the application.
pub struct OcrState {
    /// Atomic flag set by the UI when an OCR screenshot capture is requested.
    /// Polled and cleared by the OpenGL paint callback.
    pub capture_requested: Arc<AtomicBool>,
    /// Whether an OCR request is currently being processed by the background worker.
    pub is_processing: Arc<AtomicBool>,
    /// List of OCR text boxes detected from the last scan.
    pub boxes: Vec<OcrBox>,
    /// Raw unmerged lines from the last OCR scan.
    pub raw_lines: Vec<ParsedLine>,
    /// Maximum distance threshold (as a multiple of line height) for merging
    /// adjacent lines into a single text block.
    pub sticky_distance: f32,
    /// Number of seconds before OCR scan results automatically clear (0 = disabled).
    pub timeout_seconds: u32,
    /// Timestamp when the most recent OCR scan results were received.
    pub last_scan_time: Option<Instant>,
    /// Whether to hide the visual OCR overlay (boxes and replacement text) on the video feed.
    pub hide_overlay: bool,
    /// Index of the most recently clicked/copied box, for visual feedback.
    pub last_copied_index: Option<usize>,
    /// Timestamp when the box was copied, to expire visual highlight.
    pub copy_feedback_time: Option<Instant>,
    /// Last error message, if any.
    pub last_error: Option<String>,
    /// Sender for OCR worker results.
    pub ocr_sender: Option<crossbeam_channel::Sender<Result<Vec<ParsedLine>, String>>>,
    /// Channel receiver for completed OCR results from the background worker.
    pub result_receiver: Option<crossbeam_channel::Receiver<Result<Vec<ParsedLine>, String>>>,
}

impl OcrState {
    /// Recomputes merged `boxes` from `raw_lines` using the current `sticky_distance`.
    pub fn recompute_boxes(&mut self) {
        self.boxes = crate::ocr::lens::group_lines_into_blocks(&self.raw_lines, self.sticky_distance);
        self.last_copied_index = None;
    }

    /// Clears all detected OCR boxes, lines, and related state.
    pub fn clear(&mut self) {
        self.boxes.clear();
        self.raw_lines.clear();
        self.last_copied_index = None;
        self.last_scan_time = None;
    }

    /// Returns true if the OCR boxes have expired according to `timeout_seconds`.
    pub fn is_expired(&self) -> bool {
        if self.timeout_seconds == 0 || self.boxes.is_empty() {
            return false;
        }
        if let Some(scan_time) = self.last_scan_time {
            scan_time.elapsed() >= std::time::Duration::from_secs(self.timeout_seconds as u64)
        } else {
            false
        }
    }

    /// Returns the remaining duration until the OCR boxes expire, or None if expired or disabled.
    pub fn remaining_time(&self) -> Option<std::time::Duration> {
        if self.timeout_seconds == 0 || self.boxes.is_empty() {
            return None;
        }
        let scan_time = self.last_scan_time?;
        let timeout = std::time::Duration::from_secs(self.timeout_seconds as u64);
        timeout.checked_sub(scan_time.elapsed())
    }
}

impl Default for OcrState {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            capture_requested: Arc::new(AtomicBool::new(false)),
            is_processing: Arc::new(AtomicBool::new(false)),
            boxes: Vec::new(),
            raw_lines: Vec::new(),
            sticky_distance: 0.6,
            timeout_seconds: 45,
            last_scan_time: None,
            hide_overlay: false,
            last_copied_index: None,
            copy_feedback_time: None,
            last_error: None,
            ocr_sender: Some(tx),
            result_receiver: Some(rx),
        }
    }
}

// ============================================================================
// Google Lens Protobuf Schemas
// ============================================================================

#[derive(Clone, PartialEq, Message)]
pub struct CenterRotatedBox {
    #[prost(float, tag = "1")]
    pub center_x: f32,
    #[prost(float, tag = "2")]
    pub center_y: f32,
    #[prost(float, tag = "3")]
    pub width: f32,
    #[prost(float, tag = "4")]
    pub height: f32,
    #[prost(float, tag = "5")]
    pub rotation_z: f32,
    #[prost(int32, tag = "6")]
    pub coordinate_type: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Geometry {
    #[prost(message, optional, tag = "1")]
    pub bounding_box: Option<CenterRotatedBox>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextEntityIdentifier {
    #[prost(int64, tag = "1")]
    pub id: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutWord {
    #[prost(message, optional, tag = "1")]
    pub id: Option<TextEntityIdentifier>,
    #[prost(string, tag = "2")]
    pub plain_text: String,
    #[prost(string, optional, tag = "3")]
    pub text_separator: Option<String>,
    #[prost(message, optional, tag = "4")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutLine {
    #[prost(message, repeated, tag = "1")]
    pub words: Vec<TextLayoutWord>,
    #[prost(message, optional, tag = "2")]
    pub geometry: Option<Geometry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayoutParagraph {
    #[prost(message, optional, tag = "1")]
    pub id: Option<TextEntityIdentifier>,
    #[prost(message, repeated, tag = "2")]
    pub lines: Vec<TextLayoutLine>,
    #[prost(message, optional, tag = "3")]
    pub geometry: Option<Geometry>,
    #[prost(int32, tag = "4")]
    pub writing_direction: i32,
    #[prost(string, tag = "5")]
    pub content_language: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct TextLayout {
    #[prost(message, repeated, tag = "1")]
    pub paragraphs: Vec<TextLayoutParagraph>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Text {
    #[prost(message, optional, tag = "1")]
    pub text_layout: Option<TextLayout>,
    #[prost(string, tag = "2")]
    pub content_language: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageMetadata {
    #[prost(int32, tag = "1")]
    pub width: i32,
    #[prost(int32, tag = "2")]
    pub height: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImagePayload {
    #[prost(bytes, tag = "1")]
    pub image_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageData {
    #[prost(message, optional, tag = "1")]
    pub payload: Option<ImagePayload>,
    #[prost(message, optional, tag = "3")]
    pub image_metadata: Option<ImageMetadata>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppliedFilter {
    #[prost(int32, tag = "1")]
    pub filter_type: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppliedFilters {
    #[prost(message, repeated, tag = "1")]
    pub filter: Vec<AppliedFilter>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LocaleContext {
    #[prost(string, tag = "1")]
    pub language: String,
    #[prost(string, tag = "2")]
    pub region: String,
    #[prost(string, tag = "3")]
    pub time_zone: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayClientContext {
    #[prost(int32, tag = "1")]
    pub platform: i32,
    #[prost(int32, tag = "2")]
    pub surface: i32,
    #[prost(message, optional, tag = "4")]
    pub locale_context: Option<LocaleContext>,
    #[prost(string, tag = "6")]
    pub app_id: String,
    #[prost(message, optional, tag = "17")]
    pub client_filters: Option<AppliedFilters>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayRequestId {
    #[prost(uint64, tag = "1")]
    pub uuid: u64,
    #[prost(int32, tag = "2")]
    pub sequence_id: i32,
    #[prost(int32, tag = "3")]
    pub image_sequence_id: i32,
    #[prost(bytes, tag = "4")]
    pub analytics_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayRequestContext {
    #[prost(message, optional, tag = "3")]
    pub request_id: Option<LensOverlayRequestId>,
    #[prost(message, optional, tag = "4")]
    pub client_context: Option<LensOverlayClientContext>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayObjectsRequest {
    #[prost(message, optional, tag = "1")]
    pub request_context: Option<LensOverlayRequestContext>,
    #[prost(message, optional, tag = "3")]
    pub image_data: Option<ImageData>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayServerRequest {
    #[prost(message, optional, tag = "1")]
    pub objects_request: Option<LensOverlayObjectsRequest>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayObjectsResponse {
    #[prost(message, optional, tag = "3")]
    pub text: Option<Text>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LensOverlayServerResponse {
    #[prost(message, optional, tag = "2")]
    pub objects_response: Option<LensOverlayObjectsResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_serialization_roundtrip() {
        let req = LensOverlayServerRequest {
            objects_request: Some(LensOverlayObjectsRequest {
                request_context: Some(LensOverlayRequestContext {
                    request_id: Some(LensOverlayRequestId {
                        uuid: 123456789,
                        sequence_id: 0,
                        image_sequence_id: 0,
                        analytics_id: vec![1, 2, 3, 4],
                    }),
                    client_context: Some(LensOverlayClientContext {
                        platform: 3,
                        surface: 4,
                        locale_context: Some(LocaleContext {
                            language: "ja".to_string(),
                            region: "JP".to_string(),
                            time_zone: "Asia/Tokyo".to_string(),
                        }),
                        app_id: String::new(),
                        client_filters: Some(AppliedFilters {
                            filter: vec![AppliedFilter { filter_type: 7 }],
                        }),
                    }),
                }),
                image_data: Some(ImageData {
                    payload: Some(ImagePayload {
                        image_bytes: vec![0x89, 0x50, 0x4E, 0x47],
                    }),
                    image_metadata: Some(ImageMetadata {
                        width: 100,
                        height: 100,
                    }),
                }),
            }),
        };

        let mut buf = Vec::new();
        req.encode(&mut buf).expect("Failed to encode request");
        assert!(!buf.is_empty());

        let decoded = LensOverlayServerRequest::decode(&buf[..]).expect("Failed to decode");
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_ocr_state_clear() {
        let mut state = OcrState::default();
        state.boxes.push(OcrBox {
            text: "Hello".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
        });
        state.raw_lines.push(ParsedLine {
            text: "Hello".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            paragraph_idx: 0,
        });
        state.last_copied_index = Some(0);
        state.last_scan_time = Some(Instant::now());

        state.clear();
        assert!(state.boxes.is_empty());
        assert!(state.raw_lines.is_empty());
        assert_eq!(state.last_copied_index, None);
        assert_eq!(state.last_scan_time, None);
    }

    #[test]
    fn test_ocr_state_is_expired_and_remaining_time() {
        let mut state = OcrState::default();
        state.timeout_seconds = 45;

        // When boxes is empty, never expired
        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Add a box
        state.boxes.push(OcrBox {
            text: "Test".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
        });

        // If last_scan_time is None, not expired
        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Just scanned now
        state.last_scan_time = Some(Instant::now());
        assert!(!state.is_expired());
        assert!(state.remaining_time().unwrap().as_secs() <= 45);

        // Simulated scan 50 seconds ago (exceeds 45s)
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(50));
        assert!(state.is_expired());
        assert_eq!(state.remaining_time(), None);

        // Simulated scan 30 seconds ago (15s remaining)
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(30));
        assert!(!state.is_expired());
        let rem = state.remaining_time().unwrap();
        assert!(rem.as_secs() >= 14 && rem.as_secs() <= 15);
    }

    #[test]
    fn test_ocr_state_timeout_disabled() {
        let mut state = OcrState::default();
        state.timeout_seconds = 0; // Disabled
        state.boxes.push(OcrBox {
            text: "Permanent".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
        });
        state.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(9999));

        assert!(!state.is_expired());
        assert_eq!(state.remaining_time(), None);
    }
}
