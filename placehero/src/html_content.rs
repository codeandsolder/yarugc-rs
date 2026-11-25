// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseBehavior {
    None,
    Hidden,
    Ellipsis,
    Paragraph,
}

#[derive(Debug)]
struct Tag {
    name: Vec<u8>,
    close: CloseBehavior,
}

#[derive(Default)]
struct Plaintext {
    result: String,
    stack: Vec<Tag>,
    hidden: usize,
}

impl Plaintext {
    fn emit(&mut self, text: &str) {
        if self.hidden == 0 {
            self.result.push_str(text);
        }
    }

    fn start(&mut self, tag: html5gum::StartTag<()>) {
        let name = tag.name.0;
        let close = match name.as_slice() {
            b"p" => CloseBehavior::Paragraph,
            b"br" => {
                self.emit("\n");
                return;
            }
            b"span" => {
                let classes = tag.attributes.get(b"class".as_slice());
                let mut hidden = false;
                let mut ellipsis = false;
                if let Some(classes) = classes {
                    for class in String::from_utf8_lossy(&classes.value.0).split_ascii_whitespace()
                    {
                        match class {
                            "invisible" => hidden = true,
                            "ellipsis" => ellipsis = true,
                            _ => tracing::trace!(class, "Ignoring span class"),
                        }
                    }
                }
                match (hidden, ellipsis) {
                    (true, _) => CloseBehavior::Hidden,
                    (false, true) => CloseBehavior::Ellipsis,
                    _ => CloseBehavior::None,
                }
            }
            // HTML void tags never have corresponding closing tags.
            b"area" | b"base" | b"col" | b"embed" | b"hr" | b"img" | b"input" | b"link"
            | b"meta" | b"param" | b"source" | b"track" | b"wbr" => return,
            _ => CloseBehavior::None,
        };
        if tag.self_closing {
            match close {
                CloseBehavior::Paragraph => self.emit("\n\n"),
                CloseBehavior::Ellipsis => self.emit("..."),
                _ => {}
            }
            return;
        }
        if close == CloseBehavior::Hidden {
            self.hidden += 1;
        }
        self.stack.push(Tag { name, close });
    }

    fn end(&mut self, name: &[u8]) {
        let Some(index) = self.stack.iter().rposition(|tag| tag.name == name) else {
            tracing::debug!(tag = ?String::from_utf8_lossy(name), "Ignoring unmatched closing tag");
            return;
        };
        while self.stack.len() > index {
            let Some(tag) = self.stack.pop() else {
                break;
            };
            match tag.close {
                CloseBehavior::Hidden => self.hidden -= 1,
                CloseBehavior::Ellipsis => self.emit("..."),
                CloseBehavior::Paragraph => self.emit("\n\n"),
                CloseBehavior::None => {}
            }
        }
    }
}

/// Convert Mastodon status HTML into displayable plain text.
///
/// Handles paragraphs, line breaks, link ellipses, invisible spans and entities.
/// Malformed or unsupported markup never causes a panic.
pub(crate) fn status_html_to_plaintext(content: &str) -> String {
    let _span = tracing::info_span!("handle_content_html").entered();
    let mut text = Plaintext {
        result: String::with_capacity(content.len()),
        ..Default::default()
    };
    for Ok(token) in html5gum::Tokenizer::new(content) {
        match token {
            html5gum::Token::StartTag(tag) => text.start(tag),
            html5gum::Token::EndTag(tag) => text.end(&tag.name.0),
            html5gum::Token::String(value) => text.emit(&String::from_utf8_lossy(&value.value.0)),
            html5gum::Token::Error(error) => tracing::debug!(?error, "Malformed HTML token"),
            html5gum::Token::Comment(_) | html5gum::Token::Doctype(_) => {}
        }
    }
    text.result.truncate(text.result.trim_end().len());
    text.result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastodon_links_and_paragraphs() {
        assert_eq!(
            status_html_to_plaintext(
                "<p>A &amp; B<br>hello</p><p><a><span class='invisible'>https://</span><span class='ellipsis'>example</span><span class='invisible'>.com</span></a></p>"
            ),
            "A & B\nhello\n\nexample..."
        );
    }

    #[test]
    fn nested_hidden_spans_and_unmatched_tags() {
        assert_eq!(
            status_html_to_plaintext(
                "a<span class='invisible'>x<br><span class='invisible'>y</span>z</span></bad>b<img>c"
            ),
            "abc"
        );
    }
}
