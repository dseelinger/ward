//! Server-sent events, split out because it is the one part of talking to the
//! model that can be tested without a key, a network or a bill.
//!
//! The wire format is a stream of blocks separated by a blank line, where each
//! block carries one or more `field: value` lines. Only `data:` is read here —
//! the `event:` line repeats what the JSON already says in its `type` field, so
//! it is discarded.
//!
//! Bytes do not arrive on event boundaries. A chunk from the socket can end in
//! the middle of a word, so the parser keeps a buffer and yields only complete
//! blocks.

/// Accumulates bytes and hands back complete `data:` payloads.
#[derive(Default)]
pub struct Parser {
    buf: String,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk from the socket. Returns the payload of every complete
    /// event now available, in order.
    ///
    /// A block with no `data:` line — a bare comment, or the keep-alive the
    /// server sends to stop an idle connection being closed — yields nothing
    /// rather than an empty string, so callers never see a payload that is not
    /// really there.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);

        let mut out = Vec::new();

        // A block ends at a blank line. Tolerate both line endings: the
        // separator is \n\n or \r\n\r\n depending on what is between us and
        // the server.
        while let Some((end, sep_len)) = find_block_end(&self.buf) {
            let block: String = self.buf[..end].to_string();
            self.buf.drain(..end + sep_len);

            if let Some(data) = data_of(&block) {
                out.push(data);
            }
        }

        out
    }
}

fn find_block_end(s: &str) -> Option<(usize, usize)> {
    let lf = s.find("\n\n").map(|i| (i, 2));
    let crlf = s.find("\r\n\r\n").map(|i| (i, 4));

    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Joins the `data:` lines of one block. The spec allows a payload to be split
/// across several `data:` lines, joined with newlines.
fn data_of(block: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();

    for line in block.lines() {
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        // One optional space after the colon is part of the framing, not the
        // payload. Any further spaces are the payload's own.
        parts.push(rest.strip_prefix(' ').unwrap_or(rest));
    }

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yields_one_payload_per_complete_block() {
        let mut p = Parser::new();
        let got = p.push("event: ping\ndata: {\"a\":1}\n\n");
        assert_eq!(got, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn holds_a_block_until_it_is_complete() {
        let mut p = Parser::new();

        // The socket split this payload mid-token. Nothing should come out yet.
        assert!(p.push("data: {\"partial\":tr").is_empty());
        assert!(p.push("ue}").is_empty());

        assert_eq!(p.push("\n\n"), vec![r#"{"partial":true}"#]);
    }

    #[test]
    fn yields_several_blocks_from_one_chunk() {
        let mut p = Parser::new();
        let got = p.push("data: one\n\ndata: two\n\ndata: three\n\n");
        assert_eq!(got, vec!["one", "two", "three"]);
    }

    #[test]
    fn accepts_carriage_returns() {
        let mut p = Parser::new();
        let got = p.push("event: x\r\ndata: {\"a\":1}\r\n\r\n");
        assert_eq!(got, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn ignores_a_block_carrying_no_data_line() {
        let mut p = Parser::new();
        // A keep-alive comment. Yields nothing at all, rather than an empty
        // payload that a caller would try to parse as JSON.
        assert!(p.push(": keep-alive\n\n").is_empty());
    }

    #[test]
    fn joins_a_payload_split_across_data_lines() {
        let mut p = Parser::new();
        let got = p.push("data: line one\ndata: line two\n\n");
        assert_eq!(got, vec!["line one\nline two"]);
    }

    #[test]
    fn keeps_spaces_beyond_the_first() {
        let mut p = Parser::new();
        // Exactly one space after the colon is framing. A second belongs to
        // the payload, and eating it would corrupt text deltas — which is how
        // a reply loses the spaces between its words.
        let got = p.push("data:  leading\n\n");
        assert_eq!(got, vec![" leading"]);
    }

    #[test]
    fn survives_a_blank_line_arriving_on_its_own() {
        let mut p = Parser::new();
        assert!(p.push("data: a").is_empty());
        assert_eq!(p.push("\n"), Vec::<String>::new());
        assert_eq!(p.push("\n"), vec!["a"]);
    }
}
