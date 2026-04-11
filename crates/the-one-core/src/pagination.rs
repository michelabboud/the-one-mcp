//! Cursor-based pagination primitives for list/search endpoints.
//!
//! Prior to v0.15.0 the-one-mcp's list endpoints silently capped results at
//! hardcoded limits (200 for diary/lessons/audit, 2 000 for navigation nodes,
//! and unbounded for navigation tunnels). Clients had no way to detect
//! truncation and no way to page through more results. This module provides
//! the primitives to fix that class of bug with a uniform, testable API.
//!
//! # Cursor format
//!
//! Cursors are opaque base-64 encoded JSON blobs. The opacity is deliberate —
//! clients should never parse them, and we retain the freedom to change the
//! internal shape. Today the shape is:
//!
//! ```text
//! {"o": <u64 offset>, "t": <optional tiebreaker string>}
//! ```
//!
//! The tiebreaker lets endpoints that order by a non-unique key (e.g. updated
//! timestamp) remain stable across pages even if rows are inserted between
//! requests.
//!
//! # Limit enforcement
//!
//! Every list/search endpoint declares a `MAX_PAGE_SIZE`. Requests exceeding
//! the max are rejected with `CoreError::InvalidRequest`, NOT silently
//! truncated. This is a behavior change from v0.14.x — documented in the
//! changelog.
//!
//! # Response shape
//!
//! Endpoints return a `Page<T>` which carries:
//!
//! - `items: Vec<T>`
//! - `next_cursor: Option<Cursor>` — None iff this is the last page
//! - `total_count: Option<u64>` — populated for endpoints where it's cheap
//!
//! Clients detect truncation by checking `next_cursor.is_some()`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Default per-page limit when the caller omits an explicit value.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// Hard upper bound applied on top of every endpoint's own `MAX_PAGE_SIZE`.
/// No endpoint should declare a larger page size than this; if it does,
/// [`Pagination::validate_limit`] will still reject requests over this cap.
pub const GLOBAL_MAX_PAGE_SIZE: usize = 1_000;

/// Opaque pagination cursor. Clients should pass this back verbatim; they
/// must not attempt to parse the inner bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor(pub String);

/// Wire representation of the cursor — private so we can change it without
/// breaking clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorPayload {
    /// Absolute row offset into the sorted result set.
    #[serde(rename = "o")]
    offset: u64,
    /// Optional tiebreaker for non-unique sort keys.
    #[serde(rename = "t", skip_serializing_if = "Option::is_none", default)]
    tiebreaker: Option<String>,
}

impl Cursor {
    /// Build a cursor from an absolute offset.
    pub fn from_offset(offset: u64) -> Self {
        Self::encode(CursorPayload {
            offset,
            tiebreaker: None,
        })
    }

    /// Build a cursor with an offset and a tiebreaker string. Use this when
    /// the underlying query orders by a non-unique key.
    pub fn from_offset_with_tiebreaker(offset: u64, tiebreaker: impl Into<String>) -> Self {
        Self::encode(CursorPayload {
            offset,
            tiebreaker: Some(tiebreaker.into()),
        })
    }

    /// Parse an opaque cursor string. Returns `Err(InvalidRequest)` if the
    /// cursor is malformed — callers should surface this as a 400-equivalent.
    pub fn decode(value: &str) -> Result<(u64, Option<String>), CoreError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value.as_bytes())
            .map_err(|_| CoreError::InvalidRequest("cursor is not valid base64".to_string()))?;
        let payload: CursorPayload = serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::InvalidRequest("cursor payload is malformed".to_string()))?;
        Ok((payload.offset, payload.tiebreaker))
    }

    /// Return the raw cursor string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn encode(payload: CursorPayload) -> Self {
        let json = serde_json::to_vec(&payload).expect("cursor payload serializes");
        Self(URL_SAFE_NO_PAD.encode(json))
    }
}

/// A single page of results returned by a paginated endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Opaque cursor for the next page. `None` iff this is the final page.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_cursor: Option<Cursor>,
    /// Optional total count for clients that want to render progress bars.
    /// Endpoints omit this when computing the total is expensive.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_count: Option<u64>,
}

impl<T> Page<T> {
    /// Construct a page with explicit continuation info.
    pub fn new(items: Vec<T>, next_cursor: Option<Cursor>, total_count: Option<u64>) -> Self {
        Self {
            items,
            next_cursor,
            total_count,
        }
    }

    /// Shorthand for a terminal page with no continuation.
    pub fn final_page(items: Vec<T>, total_count: Option<u64>) -> Self {
        Self::new(items, None, total_count)
    }

    /// Construct a page from a raw vec + the declared per-page limit + a
    /// hint about the total row count.
    ///
    /// The `query` pattern we use everywhere: fetch `limit + 1` rows. If the
    /// query returned more than `limit` rows, we know there is at least one
    /// more page. We truncate the extra row and emit a `next_cursor` at
    /// `offset + limit`.
    pub fn from_peek(
        mut items: Vec<T>,
        limit: usize,
        current_offset: u64,
        total_count: Option<u64>,
    ) -> Self {
        if items.len() > limit {
            items.truncate(limit);
            let next_offset = current_offset.saturating_add(limit as u64);
            Self {
                items,
                next_cursor: Some(Cursor::from_offset(next_offset)),
                total_count,
            }
        } else {
            Self {
                items,
                next_cursor: None,
                total_count,
            }
        }
    }
}

/// Pagination request helper used by broker methods. Wraps cursor decoding
/// and limit validation so every endpoint has identical semantics.
#[derive(Debug, Clone)]
pub struct PageRequest {
    pub limit: usize,
    pub offset: u64,
    pub tiebreaker: Option<String>,
}

impl PageRequest {
    /// Decode a pagination request. Produces `CoreError::InvalidRequest` on
    /// invalid cursors or over-limit requests — **never silently truncates**.
    ///
    /// - `raw_limit`: the limit supplied by the client. If zero, defaults to
    ///   `default_limit`. Values greater than `max_limit` are rejected.
    /// - `cursor`: opaque cursor from a previous response, or None for first page.
    /// - `default_limit`: what to use when `raw_limit == 0`.
    /// - `max_limit`: per-endpoint hard cap (capped by [`GLOBAL_MAX_PAGE_SIZE`]).
    pub fn decode(
        raw_limit: usize,
        cursor: Option<&str>,
        default_limit: usize,
        max_limit: usize,
    ) -> Result<Self, CoreError> {
        let effective_max = max_limit.clamp(1, GLOBAL_MAX_PAGE_SIZE);
        let limit = if raw_limit == 0 {
            default_limit.min(effective_max)
        } else if raw_limit > effective_max {
            return Err(CoreError::InvalidRequest(format!(
                "limit {raw_limit} exceeds maximum of {effective_max} for this endpoint (request fewer items or page with a cursor)",
            )));
        } else {
            raw_limit
        };

        let (offset, tiebreaker) = match cursor {
            Some(c) => Cursor::decode(c)?,
            None => (0, None),
        };

        Ok(Self {
            limit,
            offset,
            tiebreaker,
        })
    }

    /// Number of rows to fetch from the database to know whether there is a
    /// next page (one extra, used by [`Page::from_peek`]).
    pub fn fetch_limit(&self) -> usize {
        self.limit.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let cursor = Cursor::from_offset(42);
        let (offset, tie) = Cursor::decode(cursor.as_str()).unwrap();
        assert_eq!(offset, 42);
        assert!(tie.is_none());
    }

    #[test]
    fn cursor_with_tiebreaker() {
        let cursor = Cursor::from_offset_with_tiebreaker(7, "abc");
        let (offset, tie) = Cursor::decode(cursor.as_str()).unwrap();
        assert_eq!(offset, 7);
        assert_eq!(tie.as_deref(), Some("abc"));
    }

    #[test]
    fn cursor_rejects_invalid_base64() {
        assert!(Cursor::decode("not base64 !!!").is_err());
    }

    #[test]
    fn cursor_rejects_invalid_payload() {
        let garbage = URL_SAFE_NO_PAD.encode(b"not json");
        assert!(Cursor::decode(&garbage).is_err());
    }

    #[test]
    fn page_request_defaults_when_zero() {
        let req = PageRequest::decode(0, None, 20, 100).unwrap();
        assert_eq!(req.limit, 20);
        assert_eq!(req.offset, 0);
    }

    #[test]
    fn page_request_rejects_over_limit() {
        let err = PageRequest::decode(500, None, 20, 100).unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }

    #[test]
    fn page_request_accepts_exact_limit() {
        let req = PageRequest::decode(100, None, 20, 100).unwrap();
        assert_eq!(req.limit, 100);
    }

    #[test]
    fn page_request_accepts_cursor() {
        let cursor = Cursor::from_offset(50);
        let req = PageRequest::decode(10, Some(cursor.as_str()), 20, 100).unwrap();
        assert_eq!(req.offset, 50);
        assert_eq!(req.limit, 10);
    }

    #[test]
    fn page_from_peek_emits_next_cursor_when_more_exists() {
        let rows: Vec<i32> = (0..11).collect();
        let page = Page::from_peek(rows, 10, 0, None);
        assert_eq!(page.items.len(), 10);
        assert!(page.next_cursor.is_some());
        let (offset, _) = Cursor::decode(page.next_cursor.unwrap().as_str()).unwrap();
        assert_eq!(offset, 10);
    }

    #[test]
    fn page_from_peek_final_page_has_no_cursor() {
        let rows: Vec<i32> = (0..5).collect();
        let page = Page::from_peek(rows, 10, 0, None);
        assert_eq!(page.items.len(), 5);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn global_max_is_respected() {
        let max = GLOBAL_MAX_PAGE_SIZE + 10;
        let err = PageRequest::decode(max, None, 20, max).unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)));
    }
}
