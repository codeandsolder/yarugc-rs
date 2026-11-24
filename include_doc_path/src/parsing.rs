// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

// ---------------------------------------------------------------------------
// String literal parsing helpers, adapted and modified from the syn crate (v2.0.115).
// Original source: https://docs.rs/syn/2.0.115/src/syn/lit.rs.html
//
// syn is dual-licensed under Apache-2.0 OR MIT.
// Copyright (c) syn contributors.
// ---------------------------------------------------------------------------

use super::compile_error;
use proc_macro::{TokenStream, TokenTree};
use proc_macro2::Span;

fn byte(s: &str, idx: usize) -> u8 {
    s.as_bytes().get(idx).copied().unwrap_or(0)
}

fn next_chr(s: &str) -> char {
    s.chars().next().unwrap_or('\0')
}

fn backslash_x(s: &str) -> Option<(u8, &str)> {
    let b0 = byte(s, 0);
    let b1 = byte(s, 1);
    let hi = char::from(b0).to_digit(16)?;
    let lo = char::from(b1).to_digit(16)?;
    let ch = u8::try_from(hi * 0x10 + lo).ok()?;
    Some((ch, &s[2..]))
}

fn backslash_u(s: &str) -> Option<(char, &str)> {
    if byte(s, 0) != b'{' {
        return None;
    }
    let s = &s[1..];
    let mut codepoint = 0;
    let mut digits = 0;
    for (index, byte) in s.bytes().enumerate() {
        match byte {
            b'}' if digits > 0 => return Some((char::from_u32(codepoint)?, s.get(index + 1..)?)),
            b'_' if digits > 0 => {}
            _ if digits < 6 => {
                codepoint = codepoint * 0x10 + char::from(byte).to_digit(16)?;
                digits += 1;
            }
            _ => return None,
        }
    }
    None
}

/// Parse a cooked (non-raw) string literal, handling all escape sequences.
///
/// `s` must be the full token text including the surrounding double-quotes.
/// Returns `(content, suffix)` on success, where `suffix` is any trailing
/// literal suffix (always empty for valid Rust string literals).
fn parse_lit_str_cooked(mut s: &str) -> Option<(Box<str>, Box<str>)> {
    if byte(s, 0) != b'"' {
        return None;
    }

    s = &s[1..];

    let mut content = String::new();
    'outer: loop {
        let ch = match byte(s, 0) {
            b'"' => break,
            b'\\' => {
                let b = byte(s, 1);
                s = s.get(2..)?;
                match b {
                    b'x' => {
                        let (byte, rest) = backslash_x(s)?;
                        s = rest;
                        if byte > 0x7F {
                            // invalid \x byte in string literal
                            return None;
                        }
                        char::from(byte)
                    }
                    b'u' => {
                        let (ch, rest) = backslash_u(s)?;
                        s = rest;
                        ch
                    }
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    b'\\' => '\\',
                    b'0' => '\0',
                    b'\'' => '\'',
                    b'"' => '"',
                    b'\r' | b'\n' => loop {
                        let b = byte(s, 0);
                        match b {
                            b' ' | b'\t' | b'\n' | b'\r' => s = &s[1..],
                            _ => continue 'outer,
                        }
                    },
                    _ => {
                        // unexpected byte after backslash
                        return None;
                    }
                }
            }
            b'\r' => {
                if byte(s, 1) != b'\n' {
                    // bare carriage return not allowed in string
                    return None;
                }
                s = &s[2..];
                '\n'
            }
            _ => {
                let ch = next_chr(s);
                s = s.get(ch.len_utf8()..)?;
                ch
            }
        };
        content.push(ch);
    }

    if !s.starts_with('"') {
        return None;
    }

    let content = content.into_boxed_str();
    let suffix = s[1..].to_owned().into_boxed_str();
    Some((content, suffix))
}

/// Parse a raw string literal (`r"..."`, `r#"..."#`, etc.).
///
/// `s` must be the full token text including the `r`, optional `#` marks, and surrounding quotes.
fn parse_lit_str_raw(mut s: &str) -> Option<(Box<str>, Box<str>)> {
    if byte(s, 0) != b'r' {
        return None;
    }
    s = &s[1..];

    let mut pounds = 0;
    loop {
        match byte(s, pounds) {
            b'#' => pounds += 1,
            b'"' => break,
            _ => return None,
        }
    }
    let close = s.rfind('"')?;
    for end in s.get(close + 1..close + 1 + pounds)?.bytes() {
        if end != b'#' {
            return None;
        }
    }

    let content = s.get(pounds + 1..close)?.to_owned().into_boxed_str();
    let suffix = s[close + 1 + pounds..].to_owned().into_boxed_str();
    Some((content, suffix))
}

// ---------------------------------------------------------------------------
// End of syn-derived code.
// ---------------------------------------------------------------------------

#[expect(
    clippy::redundant_pub_crate,
    reason = "The proc-macro crate requires non-macro helpers to remain internal; unreachable_pub rejects unrestricted visibility."
)]
pub(super) fn parse_string_literal(input: TokenStream) -> Result<(String, Span), TokenStream> {
    let mut iter = input.into_iter();
    // Must have exactly one token
    let Some(token) = iter.next() else {
        return Err(compile_error("expected string literal", Span::call_site()));
    };

    if iter.next().is_some() {
        return Err(compile_error(
            "expected only one string literal",
            Span::call_site(),
        ));
    }

    match token {
        TokenTree::Literal(lit) => {
            let span: Span = lit.span().into();
            let raw = lit.to_string();

            // Try raw string literal first (r"..." or r#"..."#)
            if raw.starts_with('r') {
                match parse_lit_str_raw(&raw) {
                    Some((content, suffix)) if suffix.is_empty() => {
                        return Ok((String::from(content), span));
                    }
                    _ => {
                        return Err(compile_error("invalid raw string literal", span));
                    }
                }
            }

            // Regular (cooked) string literal
            if raw.starts_with('"') {
                match parse_lit_str_cooked(&raw) {
                    Some((content, suffix)) if suffix.is_empty() => {
                        return Ok((String::from(content), span));
                    }
                    _ => {
                        return Err(compile_error("invalid string literal", span));
                    }
                }
            }

            Err(compile_error(
                "expected a string literal, not a byte string or other literal",
                span,
            ))
        }
        _ => Err(compile_error(
            "expected string literal like \"path/file.png\"",
            Span::call_site(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{backslash_u, parse_lit_str_cooked, parse_lit_str_raw};

    #[test]
    fn unicode_escape_separators_and_invalid_scalars() {
        assert_eq!(backslash_u("{1F_600}rest"), Some(('😀', "rest")));
        for invalid in ["{}", "{_12}", "{1234567}", "{D800}", "{110000}", "{12"] {
            assert_eq!(backslash_u(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn string_literal_parsing() {
        let (content, suffix) = parse_lit_str_cooked(r#""a\x62\u{63}""#).expect("valid literal");
        assert_eq!(&*content, "abc");
        assert!(suffix.is_empty());
        let (content, suffix) = parse_lit_str_raw(r##"r#"a\b"#"##).expect("valid raw literal");
        assert_eq!(&*content, r"a\b");
        assert!(suffix.is_empty());
        assert!(parse_lit_str_raw("r#\"unterminated").is_none());
    }
}
