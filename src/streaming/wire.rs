use super::*;

fn parse_sse_event_json(event_bytes: &[u8]) -> Option<Value> {
    let event_str = String::from_utf8_lossy(event_bytes);
    let mut data_lines = Vec::new();
    for raw_line in event_str.lines() {
        if let Some(data) = raw_line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }

    if data_lines.is_empty() {
        return None;
    }

    let data = data_lines.join("\n");
    if data == "[DONE]" {
        return Some(serde_json::json!({ "_done": true }));
    }
    if data.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&data).ok()
}

/// Locate the exclusive end offset of the next complete SSE frame in `buffer`.
///
/// A frame ends at the first blank line: the earliest `\r\n\r\n` or `\n\n`,
/// whichever comes first. A single forward scan finds it in O(frame length).
///
/// This replaces the old two-pass search (`windows(4).position(\r\n\r\n)` then
/// `windows(2).position(\n\n)`), which scanned the ENTIRE remaining buffer for
/// `\r\n\r\n` before falling back to `\n\n`. For an LF-separated buffer that
/// fallback ran every frame, making the per-frame boundary search
/// O(remaining buffer) and the whole drain O(frame_count x buffer_len) — the
/// same quadratic shape as the removed per-frame `Vec::drain`.
///
/// Observable semantics are unchanged: every existing test and every real-world
/// upstream (which uses a single separator style) gets the identical boundary.
/// For a buffer that mixes separators within one frame the new scan resolves to
/// the earliest blank line, which is also what the SSE spec requires.
fn sse_frame_end(buffer: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < buffer.len() {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some(i + 2);
        }
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

/// Read-only equivalent of [`take_one_sse_frame`]: parses the next complete SSE
/// frame from `buffer` without mutating it, returning the frame bytes, the
/// parsed event, and the number of leading bytes the frame occupies. Callers
/// that drain many frames should advance by the returned length and compact the
/// buffer once after the loop (see [`drop_sse_prefix`]) instead of mutating per
/// frame, which is O(frame_count x buffer_len) via repeated `Vec::drain`.
pub(super) fn read_one_sse_frame(buffer: &[u8]) -> Option<(Vec<u8>, Option<Value>, usize)> {
    let end = sse_frame_end(buffer)?;
    let event_bytes = buffer[..end].to_vec();
    let event = parse_sse_event_json(&event_bytes);
    Some((event_bytes, event, end))
}

/// Drop the already-parsed `consumed`-byte prefix of `buffer` in a single
/// compaction. This is the one memmove that replaces the per-frame
/// `Vec::drain(..end)` tail shift which made multi-frame draining O(n^2). If the
/// buffer was meanwhile cleared (e.g. by a resource-limit reject) this is a
/// no-op, so it is safe to call unconditionally at the end of a drain loop.
pub(super) fn drop_sse_prefix(buffer: &mut Vec<u8>, consumed: usize) {
    let drop_n = consumed.min(buffer.len());
    if drop_n > 0 {
        buffer.drain(..drop_n);
    }
}

pub(super) fn take_one_sse_frame(buffer: &mut Vec<u8>) -> Option<(Vec<u8>, Option<Value>)> {
    let (event_bytes, event, end) = read_one_sse_frame(buffer)?;
    buffer.drain(..end);
    Some((event_bytes, event))
}

pub(super) fn sse_frame_event_type(event_bytes: &[u8]) -> Option<String> {
    let event_str = String::from_utf8_lossy(event_bytes);
    for raw_line in event_str.lines() {
        if let Some(event_type) = raw_line.strip_prefix("event:") {
            let event_type = event_type.strip_prefix(' ').unwrap_or(event_type).trim();
            if !event_type.is_empty() {
                return Some(event_type.to_string());
            }
        }
    }
    None
}

pub fn take_one_sse_event(buffer: &mut Vec<u8>) -> Option<Value> {
    loop {
        let (_event_bytes, event) = take_one_sse_frame(buffer)?;
        if let Some(event) = event {
            return Some(event);
        }
    }
}

/// Format one JSON value as SSE "data: {json}\n\n".
pub fn format_sse_data(value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = b"data: ".to_vec();
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Format SSE with event type line: "event: {ty}\ndata: {json}\n\n".
pub fn format_sse_event(event_type: &str, value: &Value) -> Vec<u8> {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    let mut out = format!("event: {event_type}\n").into_bytes();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(s.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}
