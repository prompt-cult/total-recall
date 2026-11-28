//! The `sid://` pointer grammar — the front end of pointer resolution.
//!
//! A pointer selects a record inside a session without ever naming a backend
//! or a connection:
//!
//! ```text
//! sid://<session-id>?record=<record-id>
//! ```
//!
//! * `<session-id>` sits in the authority slot (between `sid://` and `?`);
//!   it is hoisted there, so it cannot be omitted — an empty authority is an
//!   error.
//! * `record` is the backend-neutral key: a part key on the sqlite store, a
//!   line number on an append-only JSONL rollout. The backend itself is never
//!   encoded in the pointer.
//! * The pointer is a query, not a connection: it says what to select, never
//!   where to attach.
//!
//! Parsing is intentionally trivial — plain string splitting, no dynamic URL
//! machinery, no crate dependency. The parser is TOTAL and BOUNDED: any input
//! yields either an [`Ok`] parse or a defined [`SidUriError`], never a panic,
//! and inputs longer than [`MAX_SID_URI_BYTES`] are rejected outright.
//!
//! Rules:
//!
//! * Must start with `sid://` (lowercase, exact).
//! * The authority (session id) may not be empty.
//! * Only `record` is accepted as a query parameter; any other parameter is
//!   rejected (strict).
//! * `record=` with an empty value is rejected.
//! * More than one `record` parameter is rejected.
//! * A pointer with no query part is valid and selects the whole session
//!   (`record` is `None`); a `?` with nothing after it is an error.
//! * Any malformed or unexpected text is rejected: bare parameters without
//!   `=`, empty parameters from stray `&` separators, fragment markers,
//!   whitespace, control bytes, non-ASCII characters, and any other byte
//!   outside the token alphabet (session and record ids are restricted to
//!   ASCII alphanumerics plus `-`, `_`, and `.`).

/// Hard cap on pointer length in bytes. Anything longer is rejected with
/// [`SidUriError::TooLong`]; the parser stays O(len) and total.
pub const MAX_SID_URI_BYTES: usize = 4096;

/// The token alphabet for session ids and record ids: ASCII alphanumerics
/// plus `-`, `_`, `.`. Everything else is rejected.
fn is_valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A parsed `sid://` pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidUri {
    /// The session id from the authority slot (never empty).
    session: String,
    /// The backend-neutral record key; `None` selects the whole session.
    record: Option<String>,
}

impl SidUri {
    /// The session id.
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The record key, if the pointer names one.
    pub fn record(&self) -> Option<&str> {
        self.record.as_deref()
    }

    /// Render the pointer back to its canonical `sid://` string form.
    ///
    /// Round-trips with [`parse`]: `parse(u.format()).unwrap() == u` for
    /// every successfully parsed pointer `u`, and `parse` accepts the render
    /// of any pointer it produced.
    pub fn format(&self) -> String {
        match &self.record {
            Some(record) => format!("sid://{}?record={}", self.session, record),
            None => format!("sid://{}", self.session),
        }
    }
}

/// Why a pointer failed to parse. Every variant is produced deterministically;
/// the parser never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidUriError {
    /// Input does not start with the exact `sid://` scheme.
    MissingScheme,
    /// The authority slot (session id) is empty.
    MissingSession,
    /// A `?` is present but the query part is empty.
    MissingQuery,
    /// A query parameter other than `record` appears (strict rejection).
    UnknownQueryParam(String),
    /// `record=` is present with an empty value.
    EmptyRecord,
    /// More than one `record` parameter appears.
    MultipleRecordParams,
    /// Input exceeds [`MAX_SID_URI_BYTES`].
    TooLong,
    /// Any other malformed input (bare parameters, stray separators,
    /// characters outside the token alphabet, …). The message names the
    /// offending fragment.
    Other(String),
}

impl std::fmt::Display for SidUriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingScheme => write!(f, "pointer must start with \"sid://\""),
            Self::MissingSession => write!(f, "session id is missing (empty authority)"),
            Self::MissingQuery => {
                write!(f, "query marker '?' is present but the query is empty")
            }
            Self::UnknownQueryParam(key) => {
                write!(
                    f,
                    "unknown query parameter {key:?}; only \"record\" is allowed"
                )
            }
            Self::EmptyRecord => write!(f, "record= is present with an empty value"),
            Self::MultipleRecordParams => write!(f, "multiple record parameters are not allowed"),
            Self::TooLong => write!(f, "pointer exceeds {MAX_SID_URI_BYTES} bytes"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SidUriError {}

/// Parse a `sid://` pointer.
///
/// Total and bounded: every input yields `Ok` or a defined [`SidUriError`];
/// inputs longer than [`MAX_SID_URI_BYTES`] bytes are rejected; no input can
/// panic this function.
pub fn parse(input: &str) -> Result<SidUri, SidUriError> {
    if input.len() > MAX_SID_URI_BYTES {
        return Err(SidUriError::TooLong);
    }
    if !input.starts_with("sid://") {
        return Err(SidUriError::MissingScheme);
    }
    let rest = &input["sid://".len()..];
    let (authority, query) = match rest.split_once('?') {
        Some((authority, query)) => (authority, Some(query)),
        None => (rest, None),
    };
    if authority.is_empty() {
        return Err(SidUriError::MissingSession);
    }
    if !is_valid_token(authority) {
        return Err(SidUriError::Other(format!(
            "invalid character in session id: {authority:?}"
        )));
    }
    let mut record: Option<String> = None;
    if let Some(query) = query {
        if query.is_empty() {
            return Err(SidUriError::MissingQuery);
        }
        for pair in query.split('&') {
            if pair.is_empty() {
                return Err(SidUriError::Other(
                    "empty query parameter (stray '&' separator)".to_string(),
                ));
            }
            let (key, value) = match pair.split_once('=') {
                Some(key_value) => key_value,
                None => {
                    return Err(SidUriError::Other(format!(
                        "query parameter without '=': {pair:?}"
                    )));
                }
            };
            if key.is_empty() {
                return Err(SidUriError::Other(
                    "query parameter with an empty name".to_string(),
                ));
            }
            if key != "record" {
                return Err(SidUriError::UnknownQueryParam(key.to_string()));
            }
            if value.is_empty() {
                return Err(SidUriError::EmptyRecord);
            }
            if !is_valid_token(value) {
                return Err(SidUriError::Other(format!(
                    "invalid character in record id: {value:?}"
                )));
            }
            if record.is_some() {
                return Err(SidUriError::MultipleRecordParams);
            }
            record = Some(value.to_string());
        }
    }
    Ok(SidUri {
        session: authority.to_string(),
        record,
    })
}
