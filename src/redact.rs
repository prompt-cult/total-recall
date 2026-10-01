//! Secret redaction for the LLM prompt path.
//!
//! An LLM-backed tool ships session content to a vendor: user prompts,
//! assistant replies, thinking, tool calls and tool results. A user who pasted
//! a key, a token or a password into a session has shipped it to the vendor.
//! The redaction here is the last thing that runs before a prompt leaves the
//! process.
//!
//! Two rules govern the whole module:
//!
//! - **Under-redaction is not a bug to be tuned away.** A shape that looks
//!   like a credential is replaced, full stop. Nothing here asks whether the
//!   match is real, live, or high-entropy, because that judgement is the one
//!   that fails open: a key quietly classified as "probably not a real key" is
//!   a key that reached a vendor.
//! - **Over-redaction is a cost, not a defect.** A prompt that loses a
//!   hyphenated identifier costs a sentence of summary fidelity. A prompt that
//!   loses a key costs a rotation. When the two conflict, the marker wins.
//!
//! Replacements are `[REDACTED:<kind>]`: stable, greppable, and they name the
//! class so a user reading a summary can see that something was removed and
//! what it was. Redaction is idempotent — running it over its own output
//! changes nothing — which is what lets the prompt-assembly path redact and
//! then let [`crate::mercury`] redact again at the wire without
//! double-mangling.

/// The shortest credential body accepted after a known vendor prefix. Long
/// enough that a hyphenated word cannot reach it, short enough that every real
/// key shape clears it comfortably.
const MIN_BODY: usize = 12;

/// A credential prefix, the alphabet its body is drawn from, and the marker
/// class it maps to. Longest/most-specific prefixes come first: `sk-ant-` must
/// be tried before `sk-`, and `ctx7sk-` before both.
struct Prefix {
    text: &'static str,
    upper_only: bool,
}

/// The key shapes this tool and its users handle directly.
const VENDOR_PREFIXES: &[Prefix] = &[
    Prefix {
        text: "ctx7sk-",
        upper_only: false,
    },
    Prefix {
        text: "sk-ant-",
        upper_only: false,
    },
    Prefix {
        text: "sk-proj-",
        upper_only: false,
    },
    Prefix {
        text: "tvly-",
        upper_only: false,
    },
    Prefix {
        text: "glpat-",
        upper_only: false,
    },
    Prefix {
        text: "xoxb-",
        upper_only: false,
    },
    Prefix {
        text: "xoxp-",
        upper_only: false,
    },
    Prefix {
        text: "xoxa-",
        upper_only: false,
    },
    Prefix {
        text: "xoxr-",
        upper_only: false,
    },
    Prefix {
        text: "xoxs-",
        upper_only: false,
    },
    Prefix {
        text: "AIza",
        upper_only: false,
    },
    Prefix {
        text: "AKIA",
        upper_only: true,
    },
    Prefix {
        text: "sk-",
        upper_only: false,
    },
    Prefix {
        text: "sk_",
        upper_only: false,
    },
];

/// The GitHub token family, matched apart from [`VENDOR_PREFIXES`] so it can
/// carry its own marker class.
const GITHUB_PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// The names whose assignment carries a credential. Matched as the tail of an
/// identifier, so `INCEPTION_API_KEY` and `client_secret` are caught alongside
/// the bare word. `-` folds to `_` before comparison, making `api-key` and
/// `api_key` one name.
const SECRET_NAMES: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "secret",
    "password",
    "passwd",
    "passphrase",
    "private_key",
    "credentials",
    "credential",
    "authorization",
];

/// Words that are a scheme, not a value: in `Authorization: Bearer <token>` the
/// credential is the token, not the word `Bearer`.
const SCHEMES: &[&str] = &["bearer", "basic", "digest", "token"];

/// A JWT header always starts with the base64 of `{"`.
const JWT_HEAD: &[u8] = b"eyJ";

/// How far past a snipped PEM header we will look for trailing base64 body
/// lines. A real key is a few kilobytes; this is well past it and stops a
/// malformed header from eating a whole prompt.
const MAX_PEM_TAIL: usize = 64 * 1024;

/// The shortest value accepted for a `key = value` assignment. Below this it
/// is a word, not a credential (`Next step: document`).
const MIN_VALUE: usize = 4;

/// A byte that can appear in a credential body.
fn in_body(b: u8, upper_only: bool) -> bool {
    if upper_only {
        b.is_ascii_uppercase() || b.is_ascii_digit()
    } else {
        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
    }
}

/// A byte that can appear in an identifier: the key side of an assignment.
fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

/// A byte that cannot appear in an unquoted assignment value.
///
/// This set ends a value at the boundary of whatever follows it — `&` in a
/// query string, `}` or `"` in JSON, whitespace in prose — and, critically,
/// includes `[`. A value that is already a `[REDACTED:…]` marker can therefore
/// never be re-redacted, which is what makes the whole pass idempotent.
fn ends_value(b: u8) -> bool {
    b.is_ascii_whitespace()
        || matches!(
            b,
            b'"' | b'\'' | b',' | b';' | b'}' | b']' | b')' | b'&' | b'#' | b'[' | b'='
        )
}

fn find_sub(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn find_sub_ci(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| p + from)
}

/// A byte range of `text` to be replaced, and the class it belongs to.
type Hit = (usize, usize);

// --- PEM blocks ---

/// A `-----BEGIN … PRIVATE KEY-----` block, as a byte range.
struct Pem {
    begin: usize,
    end: usize,
}

/// Locate a PEM private-key block at or after `from`.
///
/// The `END` line is looked for first, and a block that has one ends there. A
/// block whose `END` was snipped off ends after its run of base64 body lines:
/// tool results are snipped to 500 characters, so a truncated key is the common
/// case, not the exotic one.
fn find_pem(text: &str, from: usize) -> Option<Pem> {
    let bytes = text.as_bytes();
    let begin = find_sub(bytes, b"-----BEGIN ", from)?;
    let header_end = find_sub(bytes, b"PRIVATE KEY-----", begin)? + b"PRIVATE KEY-----".len();

    if let Some(rel) = find_sub(bytes, b"-----END ", header_end) {
        let end = bytes[rel..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| rel + p + 1)
            .unwrap_or(bytes.len());
        return Some(Pem { begin, end });
    }

    let mut pos = header_end;
    if bytes.get(pos) == Some(&b'\n') {
        pos += 1;
    }
    let limit = (header_end + MAX_PEM_TAIL).min(bytes.len());
    while pos < limit {
        let line_end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| pos + p)
            .unwrap_or(limit);
        let line = &bytes[pos..line_end];
        let is_base64 = line.len() >= 16
            && line
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
        if !is_base64 {
            break;
        }
        pos = (line_end + 1).min(limit);
    }
    Some(Pem { begin, end: pos })
}

// --- Prefix shapes ---

/// A known vendor prefix standing at a token boundary, followed by a
/// credential body long enough to be a key rather than a word.
fn find_prefixed(text: &str, from: usize, prefixes: &[Prefix], marker: &str) -> Option<Hit> {
    let bytes = text.as_bytes();
    let mut at = from;
    while at < bytes.len() {
        // A prefix glued to the tail of a word (`flask-migrations`, `sk-` in
        // `task-…`) is not a credential. A key in prose sits after a space, a
        // quote or a bracket, which is exactly what this test admits.
        if at > 0 && bytes[at - 1].is_ascii_alphanumeric() {
            at += 1;
            continue;
        }
        for p in prefixes {
            let body_start = at + p.text.len();
            if text[at..].starts_with(p.text)
                && let Some(end) = body_end(bytes, body_start, p.upper_only)
            {
                let _ = marker;
                return Some((at, end));
            }
        }
        at += 1;
    }
    None
}

fn find_vendor_key(text: &str, from: usize) -> Option<Hit> {
    find_prefixed(text, from, VENDOR_PREFIXES, "vendor-key")
}

fn find_github_token(text: &str, from: usize) -> Option<Hit> {
    let prefixes: Vec<Prefix> = GITHUB_PREFIXES
        .iter()
        .map(|t| Prefix {
            text: t,
            upper_only: false,
        })
        .collect();
    find_prefixed(text, from, &prefixes, "github-token")
}

/// Consume a credential body, returning the byte past it, or `None` when it is
/// shorter than [`MIN_BODY`] and so a word rather than a key.
fn body_end(bytes: &[u8], mut pos: usize, upper_only: bool) -> Option<usize> {
    let start = pos;
    while pos < bytes.len() && in_body(bytes[pos], upper_only) {
        pos += 1;
    }
    (pos - start >= MIN_BODY).then_some(pos)
}

// --- Bearer and JWT ---

/// The value of a `Bearer <token>` header. Returns the range of the value
/// only, so the scheme word survives for context.
fn find_bearer(text: &str, from: usize) -> Option<Hit> {
    let bytes = text.as_bytes();
    let mut at = from;
    while let Some(rel) = find_sub_ci(bytes, b"bearer", at) {
        at = rel + b"bearer".len();
        let mut pos = at;
        while bytes.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
            pos += 1;
        }
        let start = pos;
        while bytes
            .get(pos)
            .is_some_and(|&b| in_body(b, false) || b == b'.' || b == b'~')
        {
            pos += 1;
        }
        // Eight characters is well clear of the scheme words in prose
        // ("bearer responsibility") and well under any real token.
        if pos - start >= 8 {
            return Some((start, pos));
        }
    }
    None
}

/// A three-segment JWT, anchored on its header prefix. Returns the whole
/// token's range.
fn find_jwt(text: &str, from: usize) -> Option<Hit> {
    let bytes = text.as_bytes();
    let mut at = from;
    while let Some(head) = find_sub(bytes, JWT_HEAD, at) {
        at = head + JWT_HEAD.len();
        let mut pos = at;
        while pos < bytes.len() && in_body(bytes[pos], false) {
            pos += 1;
        }
        // The header segment must be substantial enough that this is a JWT and
        // not a word beginning `eyJ`.
        if pos - at < 8 {
            continue;
        }
        let mut segments = 0;
        for want_min in [0usize, 4usize] {
            if bytes.get(pos) != Some(&b'.') {
                segments = 0;
                break;
            }
            pos += 1;
            let seg_start = pos;
            while pos < bytes.len() && in_body(bytes[pos], false) {
                pos += 1;
            }
            if pos - seg_start < want_min.max(1) {
                segments = 0;
                break;
            }
            segments += 1;
        }
        if segments == 2 {
            return Some((head, pos));
        }
    }
    None
}

// --- Assignments ---

/// The value of a `key = value`, `key: value` or `"key":"value"` assignment
/// whose key ends in a [`SECRET_NAMES`] entry. Returns the range of the value
/// only: the key name and separator survive so the model can still see which
/// field was redacted.
fn find_assignment(text: &str, from: usize) -> Option<Hit> {
    let bytes = text.as_bytes();
    let mut pos = from;
    while pos < bytes.len() {
        if !is_ident(bytes[pos]) {
            pos += 1;
            continue;
        }
        let key_start = pos;
        while pos < bytes.len() && is_ident(bytes[pos]) {
            pos += 1;
        }
        // Only the earliest position in a run is worth testing: any later
        // candidate is a suffix of the same run and its value ends in the same
        // place, so skipping the whole run on a miss is not merely safe, it
        // keeps the scan linear on a 4 MiB tool result.
        if let Some((value_start, value_end)) = parse_assignment(bytes, key_start, pos)
            && value_start > from
        {
            return Some((value_start, value_end));
        }
    }
    None
}

/// Parse a separator, an optional scheme word and a value, starting at the end
/// of an identifier run.
fn parse_assignment(bytes: &[u8], key_start: usize, key_end: usize) -> Option<Hit> {
    let key = std::str::from_utf8(&bytes[key_start..key_end]).unwrap_or("");
    if !key_is_secret(key) {
        return None;
    }
    let mut pos = key_end;
    // The closing quote of a quoted key: `"api_key":"value"`.
    if matches!(bytes.get(pos), Some(b'"') | Some(b'\'')) {
        pos += 1;
    }
    while bytes.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
        pos += 1;
    }
    if !matches!(bytes.get(pos), Some(b'=') | Some(b':')) {
        return None;
    }
    pos += 1;
    while bytes.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
        pos += 1;
    }
    // `Authorization: Bearer <token>` — the scheme is not the value.
    for scheme in SCHEMES {
        let end = pos + scheme.len();
        if bytes.len() > end
            && bytes[pos..end].eq_ignore_ascii_case(scheme.as_bytes())
            && bytes.get(end).is_some_and(|b| b.is_ascii_whitespace())
        {
            pos = end;
            while bytes.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
                pos += 1;
            }
            break;
        }
    }
    if let Some(&quote) = bytes.get(pos)
        && matches!(quote, b'"' | b'\'')
    {
        // A quoted value: the range excludes both quotes, so JSON stays
        // syntactically intact.
        let value_start = pos + 1;
        let close = bytes[value_start..].iter().position(|&b| b == quote)?;
        let value_end = value_start + close;
        return (value_end - value_start >= MIN_VALUE).then_some((value_start, value_end));
    }
    let value_start = pos;
    while pos < bytes.len() && !ends_value(bytes[pos]) {
        pos += 1;
    }
    (pos - value_start >= MIN_VALUE).then_some((value_start, pos))
}

/// Does this identifier run name a credential?
///
/// The name must be the whole run or follow a `_`, `-` or `.` inside it, so
/// `access_token` and `INCEPTION_API_KEY` match while `tokenizer` and
/// `mytoken` do not.
fn key_is_secret(key: &str) -> bool {
    let folded: String = key
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .collect();
    SECRET_NAMES.iter().any(|name| {
        let n = name.len();
        folded.len() >= n
            && folded.ends_with(name)
            && (folded.len() == n
                || !folded.as_bytes()[folded.len() - n - 1].is_ascii_alphanumeric())
    })
}

// --- The pass ---

/// Replace every credential shape in `text` with a `[REDACTED:<kind>]` marker.
///
/// Unconditional. There is no switch, no environment variable, and no
/// vendor-by-vendor opt-out: a caller that wants an unredacted prompt must not
/// be able to get one from this function.
pub fn redact_secrets(text: &str) -> String {
    let mut out = redact_pem(text);
    out = replace_all(&out, find_bearer, "[REDACTED:bearer]");
    out = replace_all(&out, find_vendor_key, "[REDACTED:vendor-key]");
    out = replace_all(&out, find_github_token, "[REDACTED:github-token]");
    out = replace_all(&out, find_jwt, "[REDACTED:jwt]");
    replace_all(&out, find_assignment, "[REDACTED:secret]")
}

/// Every PEM private-key block, left to right.
fn redact_pem(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(pem) = find_pem(text, cursor) {
        out.push_str(&text[cursor..pem.begin]);
        out.push_str("[REDACTED:pem]");
        cursor = (pem.end).max(pem.begin + 1);
        if cursor >= text.len() {
            return out;
        }
    }
    out.push_str(&text[cursor..]);
    out
}

/// Apply `find` repeatedly from `from`, replacing each match with `marker`.
///
/// The scan resumes past the match, so a marker can never be re-entered. Every
/// finder is linear in the length of the text, and each pass is guarded by a
/// cheap gate so a prompt that is a diff or a source file does not pay for six
/// full scans it cannot match.
fn replace_all<F>(text: &str, find: F, marker: &str) -> String
where
    F: Fn(&str, usize) -> Option<Hit>,
{
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some((start, end)) = find(text, cursor) {
        if start < cursor || end <= start {
            cursor = (end.max(cursor)) + 1;
            continue;
        }
        out.push_str(&text[cursor..start]);
        out.push_str(marker);
        cursor = end;
        if cursor >= text.len() {
            return out;
        }
    }
    out.push_str(&text[cursor..]);
    out
}
