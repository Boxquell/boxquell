//! Reads how many tokens a request to a model API used, from the answer as
//! it passes by: Anthropic's and OpenAI's formats, whole answers and streams.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What one request cost in tokens. `input` never includes cached tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub model: String,
    pub input: u64,
    pub output: u64,
    /// Input read from the provider's cache, billed at a lower price.
    pub cache_read: u64,
    /// Input written to the cache, billed at a higher price where the provider does so.
    pub cache_write: u64,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write == 0
    }
}

/// Looks for `"usage": {…}` objects and the model's name in the bytes of an
/// answer, fed in pieces. A stream reports its usage several times with
/// growing numbers, so the largest of each kind counts.
#[derive(Default)]
pub struct Scanner {
    pending: Vec<u8>,
    pub usage: Usage,
}

/// More than any usage object with what precedes it in one piece of a stream.
const KEEP: usize = 16 * 1024;

impl Scanner {
    pub fn feed(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        let mut from = 0;
        loop {
            let rest = &self.pending[from..];
            let usage_at = find(rest, b"\"usage\"");
            let model_at = if self.usage.model.is_empty() { find(rest, b"\"model\"") } else { None };
            match (usage_at, model_at) {
                (None, None) => break,
                (usage_at, Some(at)) if usage_at.is_none_or(|usage| at < usage) => match string_after(&rest[at + 7..]) {
                    Some(Some(name)) => {
                        self.usage.model = name;
                        from += at + 7;
                    }
                    Some(None) => from += at + 7,
                    // The name is cut off: wait for the next piece.
                    None => {
                        from += at;
                        break;
                    }
                },
                (Some(at), _) => match object_after(&rest[at + 7..]) {
                    Some(Some((object, len))) => {
                        self.take(&object);
                        from += at + 7 + len;
                    }
                    Some(None) => from += at + 7,
                    None => {
                        from += at;
                        break;
                    }
                },
                (None, Some(_)) => unreachable!("covered by the guard above"),
            }
        }
        // Keep what may be the start of something, but never the whole answer.
        let keep_from = from.max(self.pending.len().saturating_sub(KEEP));
        self.pending.drain(..keep_from);
    }

    fn take(&mut self, usage: &Value) {
        let number = |keys: &[&str]| keys.iter().find_map(|key| usage[key].as_u64());
        let cached_in_details = ["input_tokens_details", "prompt_tokens_details"].iter().find_map(|key| usage[key]["cached_tokens"].as_u64());
        let cache_read = usage["cache_read_input_tokens"].as_u64().or(cached_in_details).unwrap_or(0);
        let mut input: u64 = number(&["input_tokens", "prompt_tokens"]).unwrap_or(0);
        // OpenAI counts cached tokens as part of the input; Anthropic lists them apart.
        if cached_in_details.is_some() {
            input = input.saturating_sub(cache_read);
        }
        let mine = &mut self.usage;
        mine.input = mine.input.max(input);
        mine.output = mine.output.max(number(&["output_tokens", "completion_tokens"]).unwrap_or(0));
        mine.cache_read = mine.cache_read.max(cache_read);
        mine.cache_write = mine.cache_write.max(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0));
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// After a key: skip `:` and spaces. `None` when the bytes end first.
fn value_start(bytes: &[u8]) -> Option<usize> {
    let mut at = 0;
    while at < bytes.len() && (bytes[at] == b':' || bytes[at].is_ascii_whitespace()) {
        at += 1;
    }
    (at < bytes.len()).then_some(at)
}

/// The string value after a key. Outer `None`: cut off. Inner `None`: not a string.
fn string_after(bytes: &[u8]) -> Option<Option<String>> {
    let start = value_start(bytes)?;
    if bytes[start] != b'"' {
        return Some(None);
    }
    let body = &bytes[start + 1..];
    let end = body.iter().position(|b| *b == b'"')?;
    let name = String::from_utf8_lossy(&body[..end.min(120)]).into_owned();
    Some((!name.is_empty() && !name.contains('\\')).then_some(name))
}

/// The object value after a key, with how many bytes it took. Outer `None`: cut off.
fn object_after(bytes: &[u8]) -> Option<Option<(Value, usize)>> {
    let start = value_start(bytes)?;
    if bytes[start] != b'{' {
        return Some(None);
    }
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for (at, byte) in bytes.iter().enumerate().skip(start) {
        match (in_string, byte) {
            (true, _) if escaped => escaped = false,
            (true, b'\\') => escaped = true,
            (true, b'"') => in_string = false,
            (true, _) => {}
            (false, b'"') => in_string = true,
            (false, b'{') => depth += 1,
            (false, b'}') => {
                depth -= 1;
                if depth == 0 {
                    return Some(serde_json::from_slice(&bytes[start..=at]).ok().map(|value| (value, at + 1)));
                }
            }
            (false, _) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(pieces: &[&str]) -> Usage {
        let mut scanner = Scanner::default();
        for piece in pieces {
            scanner.feed(piece.as_bytes());
        }
        scanner.usage
    }

    #[test]
    fn reads_an_anthropic_stream_cut_in_awkward_places() {
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-x-1\",\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":100,\"cache_read_input_tokens\":900,\"output_tokens\":1}}}\n\n",
            "event: content_block_delta\ndata: {\"delta\":{\"text\":\"the \\\"usage\\\" of words\"}}\n\n",
            "event: message_delta\ndata: {\"usage\":{\"output_tokens\":345}}\n\n",
        );
        let whole = scan(&[stream]);
        assert_eq!(whole, Usage { model: "claude-x-1".into(), input: 12, output: 345, cache_read: 900, cache_write: 100 });
        for cut in [30, 75, 110, 150, stream.len() - 20] {
            assert_eq!(scan(&[&stream[..cut], &stream[cut..]]), whole, "cut at {cut}");
        }
    }

    #[test]
    fn reads_openai_answers_and_takes_cached_tokens_out_of_the_input() {
        let chat =
            r#"{"model":"gpt-x","choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":800}}}"#;
        assert_eq!(scan(&[chat]), Usage { model: "gpt-x".into(), input: 200, output: 50, cache_read: 800, cache_write: 0 });
        let responses = r#"{"type":"response.completed","response":{"model":"gpt-y","usage":{"input_tokens":70,"input_tokens_details":{"cached_tokens":0},"output_tokens":9}}}"#;
        assert_eq!(scan(&[responses]), Usage { model: "gpt-y".into(), input: 70, output: 9, cache_read: 0, cache_write: 0 });
        assert!(scan(&[r#"{"usage":null,"model":7}"#]).is_empty());
    }
}
