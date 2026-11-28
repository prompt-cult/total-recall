//! Tests for the `sid://` pointer parser (`crate::sid_uri`).
//!
//! The parser is TOTAL: every test drives inputs that yield either a valid
//! [`SidUri`] or a defined [`SidUriError`], and the pseudo-fuzz loops push
//! thousands of deterministic malformed strings through the parser, asserting
//! a calm result for each — never a panic.

use total_recall::sid_uri::{MAX_SID_URI_BYTES, SidUri, SidUriError, parse};

fn ok(input: &str) -> SidUri {
    match parse(input) {
        Ok(uri) => uri,
        Err(e) => panic!("expected Ok for {input:?}, got {e}"),
    }
}

fn err(input: &str) -> SidUriError {
    match parse(input) {
        Err(e) => e,
        Ok(uri) => panic!("expected Err for {input:?}, got Ok {uri:?}"),
    }
}

#[test]
fn valid_pointers_parse() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("sid://abc?record=42", "abc", Some("42")),
        (
            "sid://ses_e250a3909ffekzYjYwDspRzqJn?record=7",
            "ses_e250a3909ffekzYjYwDspRzqJn",
            Some("7"),
        ),
        (
            "sid://ses_e250a3909ffekzYjYwDspRzqJn",
            "ses_e250a3909ffekzYjYwDspRzqJn",
            None,
        ),
        ("sid://0?record=0", "0", Some("0")),
        ("sid://a-b_c.d?record=1", "a-b_c.d", Some("1")),
        ("sid://X?record=zz", "X", Some("zz")),
        ("sid://foo?record=001", "foo", Some("001")),
        ("sid://foo?record=1x", "foo", Some("1x")),
        ("sid://foo?record=1-2", "foo", Some("1-2")),
        ("sid://foo", "foo", None),
    ];
    for (input, session, record) in cases {
        let uri = ok(input);
        assert_eq!(uri.session(), *session, "session mismatch for {input:?}");
        assert_eq!(uri.record(), *record, "record mismatch for {input:?}");
    }
}

#[test]
fn malformed_pointers_rejected_with_exact_variant() {
    let cases: &[(&str, SidUriError)] = &[
        ("", SidUriError::MissingScheme),
        ("sid:", SidUriError::MissingScheme),
        ("sid:/", SidUriError::MissingScheme),
        ("SID://a?record=1", SidUriError::MissingScheme),
        ("sId://a?record=1", SidUriError::MissingScheme),
        (" sid://a?record=1", SidUriError::MissingScheme),
        ("sid://", SidUriError::MissingSession),
        ("sid://?", SidUriError::MissingSession),
        ("sid://?record=1", SidUriError::MissingSession),
        ("sid://a?", SidUriError::MissingQuery),
        ("sid://a?record=", SidUriError::EmptyRecord),
        ("sid://a?record=&x=1", SidUriError::EmptyRecord),
        ("sid://a?record=1&record=", SidUriError::EmptyRecord),
        (
            "sid://a?record=1&record=2",
            SidUriError::MultipleRecordParams,
        ),
        (
            "sid://a?record=1&record=2&record=3",
            SidUriError::MultipleRecordParams,
        ),
        (
            "sid://a?x=1",
            SidUriError::UnknownQueryParam("x".to_string()),
        ),
        (
            "sid://a?record=1&x=2",
            SidUriError::UnknownQueryParam("x".to_string()),
        ),
        (
            "sid://a?RECORD=1",
            SidUriError::UnknownQueryParam("RECORD".to_string()),
        ),
        (
            "sid://a?records=1",
            SidUriError::UnknownQueryParam("records".to_string()),
        ),
        (
            "sid://a?record=1&foo=2",
            SidUriError::UnknownQueryParam("foo".to_string()),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(err(input), expected.clone(), "input {input:?}");
    }
}

#[test]
fn malformed_pointers_rejected_as_other() {
    let cases: &[&str] = &[
        "sid://a?record",
        "sid://a?=1",
        "sid://a?record=1&",
        "sid://a?&record=1",
        "sid://a?record=1&&record=2",
        "sid://a?record=1#frag",
        "sid://a?record=1?x",
        "sid://a?record=1 ",
        "sid://a?record= 1",
        "sid://a?record=1\n",
        "sid://a?record=1\t",
        "sid://a?record=%31",
        "sid://a?record=1/2",
        "sid://a?record=1;2",
        "sid://a?record=1\\",
        "sid://a?record=1\0",
        "sid://a b?record=1",
        "sid://☃?record=1",
        "sid://a?record=é",
    ];
    for input in cases {
        assert!(
            matches!(err(input), SidUriError::Other(_)),
            "input {input:?} should be SidUriError::Other"
        );
    }
}

#[test]
fn format_round_trips() {
    let valid: &[&str] = &[
        "sid://abc?record=42",
        "sid://ses_e250a3909ffekzYjYwDspRzqJn?record=7",
        "sid://ses_e250a3909ffekzYjYwDspRzqJn",
        "sid://0?record=0",
        "sid://a-b_c.d?record=1",
        "sid://foo?record=1x",
        "sid://foo",
    ];
    for input in valid {
        let uri = ok(input);
        assert_eq!(uri.format(), *input, "format mismatch for {input:?}");
        assert_eq!(
            parse(&uri.format()),
            Ok(uri),
            "reparse mismatch for {input:?}"
        );
    }
}

#[test]
fn input_length_is_bounded() {
    // Exactly the cap parses; one byte over is TooLong.
    let session_len = MAX_SID_URI_BYTES - "sid://".len() - "?record=1".len();
    let at_cap = format!("sid://{}?record=1", "a".repeat(session_len));
    assert_eq!(at_cap.len(), MAX_SID_URI_BYTES);
    assert!(parse(&at_cap).is_ok());

    let over_cap = format!("sid://{}?record=1", "a".repeat(session_len + 1));
    assert_eq!(over_cap.len(), MAX_SID_URI_BYTES + 1);
    assert_eq!(err(&over_cap), SidUriError::TooLong);

    // Query-less form at the cap.
    let session_only_len = MAX_SID_URI_BYTES - "sid://".len();
    let at_cap_no_query = format!("sid://{}", "a".repeat(session_only_len));
    assert_eq!(at_cap_no_query.len(), MAX_SID_URI_BYTES);
    assert!(parse(&at_cap_no_query).is_ok());

    let over_cap_no_query = format!("sid://{}", "a".repeat(session_only_len + 1));
    assert_eq!(err(&over_cap_no_query), SidUriError::TooLong);

    // A giant blob is rejected before any other check.
    assert_eq!(err(&"a".repeat(8192)), SidUriError::TooLong);
    assert_eq!(
        err(&format!("sid://{}?record=2", "a".repeat(MAX_SID_URI_BYTES))),
        SidUriError::TooLong
    );
}

/// Deterministic 64-bit LCG for the pseudo-fuzz loops (fixed seed, so the
/// generated tables are reproducible across runs).
fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// A low-bias draw from the LCG: the low bits alternate predictably (odd
/// multiplier/increment), so decisions use the well-mixed high half instead.
fn rnd(state: &mut u64) -> u64 {
    next_random(state) >> 32
}

/// A pool of bytes designed to be hostile: valid pointer punctuation, stray
/// separators, whitespace, control bytes, and non-ASCII characters.
const FUZZ_POOL: &[char] = &[
    's', 'i', 'd', ':', '/', '?', '=', '&', 'r', 'e', 'c', 'o', '1', '2', 'x', ' ', '\n', '\0',
    '_', '-', '.', '#', '%', '\\', '@', '[', ']', '\t', 'A', 'Z', '0', '9', 'é', '☃', '~', '+',
];

#[test]
fn pseudo_fuzz_composed_strings_is_total() {
    let mut state: u64 = 0x5eed_5eed_5eed_5eed;
    let mut oks = 0usize;
    let mut errs = 0usize;
    for _ in 0..10_000 {
        // 3/4 of draws start from a valid core (scheme + random authority,
        // optionally a query) with the rest filled from the hostile pool;
        // 1/4 are fully random. This keeps both Ok and Err branches exercised
        // while still pushing a huge variety of malformed bytes through.
        let mut s = String::new();
        if !rnd(&mut state).is_multiple_of(4) {
            s.push_str("sid://");
            let auth_len = 1 + (rnd(&mut state) % 12) as usize;
            for _ in 0..auth_len {
                let idx = (rnd(&mut state) as usize) % FUZZ_POOL.len();
                s.push(FUZZ_POOL[idx]);
            }
            if rnd(&mut state).is_multiple_of(2) {
                s.push('?');
                let q_len = (rnd(&mut state) % 12) as usize;
                for _ in 0..q_len {
                    let idx = (rnd(&mut state) as usize) % FUZZ_POOL.len();
                    s.push(FUZZ_POOL[idx]);
                }
            }
        } else {
            let len = (rnd(&mut state) % 40) as usize;
            for _ in 0..len {
                let idx = (rnd(&mut state) as usize) % FUZZ_POOL.len();
                s.push(FUZZ_POOL[idx]);
            }
        }
        match parse(&s) {
            Ok(uri) => {
                oks += 1;
                // Every Ok must round-trip; a total parser never diverges here.
                assert_eq!(parse(&uri.format()), Ok(uri), "round-trip broke for {s:?}");
            }
            Err(_) => errs += 1,
        }
    }
    assert!(
        oks > 0,
        "fuzz never produced a valid parse — generator too weak"
    );
    assert!(
        errs > 0,
        "fuzz never produced a rejection — not exercising errors"
    );
    assert_eq!(oks + errs, 10_000);
}

#[test]
fn pseudo_fuzz_template_mutations_is_total() {
    let seeds: &[&str] = &[
        "sid://abc?record=42",
        "sid://ses_e250a3909ffekzYjYwDspRzqJn?record=7",
        "sid://foo",
        "sid://a-b_c.d?record=1",
        "sid://0?record=0",
    ];
    let mut state: u64 = 0x00C0_FFEE_00BA_5EBA;
    let mut oks = 0usize;
    let mut errs = 0usize;
    for _ in 0..10_000 {
        let mut s: Vec<char> = seeds[(next_random(&mut state) as usize) % seeds.len()]
            .chars()
            .collect();
        let ops = (next_random(&mut state) as usize) % 9;
        for _ in 0..ops {
            match (next_random(&mut state) as usize) % 3 {
                0 => {
                    // insert a random pool char at a random position
                    let pos = (next_random(&mut state) as usize) % (s.len() + 1);
                    let c = FUZZ_POOL[(next_random(&mut state) as usize) % FUZZ_POOL.len()];
                    s.insert(pos, c);
                }
                1 => {
                    // delete a random char
                    if !s.is_empty() {
                        let pos = (next_random(&mut state) as usize) % s.len();
                        s.remove(pos);
                    }
                }
                _ => {
                    // replace a random char
                    if !s.is_empty() {
                        let pos = (next_random(&mut state) as usize) % s.len();
                        s[pos] = FUZZ_POOL[(next_random(&mut state) as usize) % FUZZ_POOL.len()];
                    }
                }
            }
        }
        let s: String = s.into_iter().collect();
        match parse(&s) {
            Ok(uri) => {
                oks += 1;
                assert_eq!(parse(&uri.format()), Ok(uri), "round-trip broke for {s:?}");
            }
            Err(_) => errs += 1,
        }
    }
    assert!(
        oks > 0,
        "fuzz never produced a valid parse — generator too weak"
    );
    assert!(
        errs > 0,
        "fuzz never produced a rejection — not exercising errors"
    );
    assert_eq!(oks + errs, 10_000);
}
