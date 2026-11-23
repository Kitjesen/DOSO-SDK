//! Zero-dependency HTTP/1.1 + JSON bridge from `doso.v1.Voice` to an onboard AskMe runtime.
//!
//! Keeps `AskMe` bound to loopback (default `http://127.0.0.1:8765`) while exposing
//! unified voice & dialogue management through DOSO's `:5052` gateway contract.

use std::collections::BTreeMap;
use std::time::Duration;

use doso_sdk::rpc as doso;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tonic::Status;

#[derive(Debug, Clone)]
pub struct AskMeVoiceConfig {
    pub endpoint: String,
    pub operator_id: String,
    pub api_token: Option<String>,
    pub robot_id: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonVal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(BTreeMap<String, JsonVal>),
}

impl JsonVal {
    fn as_obj(&self) -> Option<&BTreeMap<String, JsonVal>> {
        match self {
            JsonVal::Obj(m) => Some(m),
            _ => None,
        }
    }
    fn as_arr(&self) -> Option<&[JsonVal]> {
        match self {
            JsonVal::Arr(a) => Some(a),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            JsonVal::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn as_bool(&self) -> Option<bool> {
        match self {
            JsonVal::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<&JsonVal> {
        self.as_obj()?.get(key)
    }
    fn get_str(&self, key: &str) -> String {
        self.get(key)
            .and_then(JsonVal::as_str)
            .unwrap_or("")
            .to_owned()
    }
    fn get_f64(&self, key: &str) -> f64 {
        self.get(key).and_then(JsonVal::as_f64).unwrap_or(0.0)
    }
    fn get_bool(&self, key: &str) -> bool {
        self.get(key).and_then(JsonVal::as_bool).unwrap_or(false)
    }
}

pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn parse_json(input: &str) -> std::result::Result<JsonVal, String> {
    let bytes = input.as_bytes();
    let mut pos = 0usize;
    let val = parse_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!("trailing JSON content at byte {pos}"));
    }
    Ok(val)
}

fn skip_ws(b: &[u8], pos: &mut usize) {
    while *pos < b.len() && matches!(b[*pos], b' ' | b'\t' | b'\r' | b'\n') {
        *pos += 1;
    }
}

fn parse_value(b: &[u8], pos: &mut usize) -> std::result::Result<JsonVal, String> {
    skip_ws(b, pos);
    if *pos >= b.len() {
        return Err("unexpected end of JSON".into());
    }
    match b[*pos] {
        b'{' => parse_object(b, pos),
        b'[' => parse_array(b, pos),
        b'"' => parse_string(b, pos).map(JsonVal::Str),
        b't' => expect_lit(b, pos, b"true", JsonVal::Bool(true)),
        b'f' => expect_lit(b, pos, b"false", JsonVal::Bool(false)),
        b'n' => expect_lit(b, pos, b"null", JsonVal::Null),
        b'-' | b'0'..=b'9' => parse_number(b, pos),
        other => Err(format!("unexpected JSON byte '{}' at {pos}", other as char)),
    }
}

fn expect_lit(
    b: &[u8],
    pos: &mut usize,
    lit: &[u8],
    val: JsonVal,
) -> std::result::Result<JsonVal, String> {
    if b[*pos..].starts_with(lit) {
        *pos += lit.len();
        Ok(val)
    } else {
        Err(format!("invalid JSON literal at {pos}"))
    }
}

fn parse_number(b: &[u8], pos: &mut usize) -> std::result::Result<JsonVal, String> {
    let start = *pos;
    if b[*pos] == b'-' {
        *pos += 1;
    }
    while *pos < b.len() && matches!(b[*pos], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
        *pos += 1;
    }
    let slice = std::str::from_utf8(&b[start..*pos]).map_err(|e| e.to_string())?;
    let n = slice
        .parse::<f64>()
        .map_err(|e| format!("invalid JSON number '{slice}': {e}"))?;
    Ok(JsonVal::Num(n))
}

fn parse_hex4(b: &[u8], pos: &mut usize) -> std::result::Result<u16, String> {
    if *pos + 4 > b.len() {
        return Err("truncated \\u escape".into());
    }
    let s = std::str::from_utf8(&b[*pos..*pos + 4]).map_err(|e| e.to_string())?;
    let v = u16::from_str_radix(s, 16).map_err(|e| format!("bad hex \\u{s}: {e}"))?;
    *pos += 4;
    Ok(v)
}

fn parse_string(b: &[u8], pos: &mut usize) -> std::result::Result<String, String> {
    if b[*pos] != b'"' {
        return Err("expected '\"'".into());
    }
    *pos += 1;
    let mut out = String::new();
    while *pos < b.len() {
        let ch = b[*pos];
        if ch == b'"' {
            *pos += 1;
            return Ok(out);
        }
        if ch == b'\\' {
            *pos += 1;
            if *pos >= b.len() {
                return Err("truncated escape".into());
            }
            match b[*pos] {
                b'"' => {
                    out.push('"');
                    *pos += 1;
                }
                b'\\' => {
                    out.push('\\');
                    *pos += 1;
                }
                b'/' => {
                    out.push('/');
                    *pos += 1;
                }
                b'b' => {
                    out.push('\u{0008}');
                    *pos += 1;
                }
                b'f' => {
                    out.push('\u{000c}');
                    *pos += 1;
                }
                b'n' => {
                    out.push('\n');
                    *pos += 1;
                }
                b'r' => {
                    out.push('\r');
                    *pos += 1;
                }
                b't' => {
                    out.push('\t');
                    *pos += 1;
                }
                b'u' => {
                    *pos += 1;
                    let code = parse_hex4(b, pos)?;
                    if (0xD800..=0xDBFF).contains(&code) {
                        if *pos + 2 <= b.len() && b[*pos] == b'\\' && b[*pos + 1] == b'u' {
                            *pos += 2;
                            let low = parse_hex4(b, pos)?;
                            let cp =
                                0x10000 + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                            if let Some(c) = char::from_u32(cp) {
                                out.push(c);
                            }
                        }
                    } else if let Some(c) = char::from_u32(code as u32) {
                        out.push(c);
                    }
                }
                other => return Err(format!("invalid escape \\{}", other as char)),
            }
            continue;
        }
        let rest = std::str::from_utf8(&b[*pos..]).map_err(|e| e.to_string())?;
        let c = rest.chars().next().ok_or("unexpected EOF in string")?;
        out.push(c);
        *pos += c.len_utf8();
    }
    Err("unterminated JSON string".into())
}

fn parse_array(b: &[u8], pos: &mut usize) -> std::result::Result<JsonVal, String> {
    *pos += 1; // '['
    let mut items = Vec::new();
    skip_ws(b, pos);
    if *pos < b.len() && b[*pos] == b']' {
        *pos += 1;
        return Ok(JsonVal::Arr(items));
    }
    loop {
        items.push(parse_value(b, pos)?);
        skip_ws(b, pos);
        if *pos >= b.len() {
            return Err("unterminated JSON array".into());
        }
        match b[*pos] {
            b',' => *pos += 1,
            b']' => {
                *pos += 1;
                return Ok(JsonVal::Arr(items));
            }
            other => return Err(format!("expected ',' or ']', got '{}'", other as char)),
        }
    }
}

fn parse_object(b: &[u8], pos: &mut usize) -> std::result::Result<JsonVal, String> {
    *pos += 1; // '{'
    let mut map = BTreeMap::new();
    skip_ws(b, pos);
    if *pos < b.len() && b[*pos] == b'}' {
        *pos += 1;
        return Ok(JsonVal::Obj(map));
    }
    loop {
        skip_ws(b, pos);
        let key = parse_string(b, pos)?;
        skip_ws(b, pos);
        if *pos >= b.len() || b[*pos] != b':' {
            return Err("expected ':' in JSON object".into());
        }
        *pos += 1;
        let val = parse_value(b, pos)?;
        map.insert(key, val);
        skip_ws(b, pos);
        if *pos >= b.len() {
            return Err("unterminated JSON object".into());
        }
        match b[*pos] {
            b',' => *pos += 1,
            b'}' => {
                *pos += 1;
                return Ok(JsonVal::Obj(map));
            }
            other => return Err(format!("expected ',' or '}}', got '{}'", other as char)),
        }
    }
}

fn parse_http_endpoint(raw: &str) -> std::result::Result<(String, u16, String), Status> {
    let trimmed = raw.trim().trim_end_matches('/');
    let rest = trimmed.strip_prefix("http://").ok_or_else(|| {
        Status::invalid_argument(format!(
            "ASKME_ENDPOINT must start with http:// (got '{trimmed}')"
        ))
    })?;
    let (host_port, base_path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_owned()),
        None => (rest, String::new()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => {
            let port = p.parse::<u16>().map_err(|_| {
                Status::invalid_argument(format!("invalid port in ASKME_ENDPOINT: '{p}'"))
            })?;
            (h.to_owned(), port)
        }
        None => (host_port.to_owned(), 80u16),
    };
    if host.is_empty() {
        return Err(Status::invalid_argument("empty host in ASKME_ENDPOINT"));
    }
    Ok((host, port, base_path))
}

fn decode_chunked(body: &[u8]) -> std::result::Result<Vec<u8>, Status> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let Some(rel_end) = body[pos..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_line = std::str::from_utf8(&body[pos..pos + rel_end])
            .map_err(|e| Status::internal(format!("bad chunk size utf8: {e}")))?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_line, 16)
            .map_err(|e| Status::internal(format!("bad chunk size '{size_line}': {e}")))?;
        pos += rel_end + 2;
        if size == 0 {
            break;
        }
        if pos + size > body.len() {
            out.extend_from_slice(&body[pos..]);
            break;
        }
        out.extend_from_slice(&body[pos..pos + size]);
        pos += size;
        if pos + 2 <= body.len() && &body[pos..pos + 2] == b"\r\n" {
            pos += 2;
        }
    }
    Ok(out)
}

async fn http_json(
    cfg: &AskMeVoiceConfig,
    method: &str,
    path: &str,
    body: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> std::result::Result<(u16, String), Status> {
    let (host, port, base_path) = parse_http_endpoint(&cfg.endpoint)?;
    let full_path = format!("{base_path}{path}");
    let addr = format!("{host}:{port}");

    let fut = async {
        let mut stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| Status::unavailable(format!("AskMe {addr} unavailable: {e}")))?;
        let mut req = format!(
            "{method} {full_path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAccept: application/json\r\n"
        );
        if !cfg.operator_id.trim().is_empty() {
            req.push_str(&format!("X-Askme-Operator-Id: {}\r\n", cfg.operator_id));
        }
        if let Some(tok) = cfg.api_token.as_deref().filter(|t| !t.trim().is_empty()) {
            req.push_str(&format!("Authorization: Bearer {}\r\n", tok.trim()));
        }
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(payload) = body {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n\r\n", payload.len()));
            req.push_str(payload);
        } else {
            req.push_str("\r\n");
        }
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| Status::unavailable(format!("AskMe write failed: {e}")))?;
        stream
            .flush()
            .await
            .map_err(|e| Status::unavailable(format!("AskMe flush failed: {e}")))?;

        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        let mut header_end_opt: Option<usize> = None;
        let mut content_length: Option<usize> = None;
        let mut is_chunked = false;
        loop {
            let n = stream
                .read(&mut buf)
                .await
                .map_err(|e| Status::unavailable(format!("AskMe read failed: {e}")))?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            if header_end_opt.is_none() {
                if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end_opt = Some(idx);
                    if let Ok(head_str) = std::str::from_utf8(&raw[..idx]) {
                        for line in head_str.lines().skip(1) {
                            let lower = line.to_ascii_lowercase();
                            if let Some(val) = lower.strip_prefix("content-length:") {
                                content_length = val.trim().parse::<usize>().ok();
                            } else if lower.starts_with("transfer-encoding:")
                                && lower.contains("chunked")
                            {
                                is_chunked = true;
                            }
                        }
                    }
                }
            }
            if let Some(h_end) = header_end_opt {
                let body_len = raw.len().saturating_sub(h_end + 4);
                if let Some(expected) = content_length {
                    if body_len >= expected {
                        break;
                    }
                } else if is_chunked && raw[h_end + 4..].windows(5).any(|w| w == b"0\r\n\r\n") {
                    break;
                }
            }
        }

        let header_end =
            header_end_opt.ok_or_else(|| Status::internal("malformed HTTP response from AskMe"))?;
        let head = std::str::from_utf8(&raw[..header_end])
            .map_err(|e| Status::internal(format!("bad HTTP header utf8: {e}")))?;
        let status_line = head
            .lines()
            .next()
            .ok_or_else(|| Status::internal("empty HTTP status line from AskMe"))?;
        let status_code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| Status::internal(format!("invalid HTTP status line: {status_line}")))?;
        let raw_body = &raw[header_end + 4..];
        let body_bytes = if is_chunked {
            decode_chunked(raw_body)?
        } else if let Some(expected) = content_length {
            raw_body[..raw_body.len().min(expected)].to_vec()
        } else {
            raw_body.to_vec()
        };
        let text = String::from_utf8_lossy(&body_bytes).into_owned();
        Ok((status_code, text))
    };

    tokio::time::timeout(cfg.timeout, fut)
        .await
        .map_err(|_| Status::deadline_exceeded("AskMe HTTP request timed out"))?
}

fn map_http_error(code: u16, body: &str) -> Status {
    let msg = parse_json(body)
        .ok()
        .and_then(|v| {
            let m = v.get_str("message");
            if !m.is_empty() {
                Some(m)
            } else {
                let e = v.get_str("error");
                if !e.is_empty() {
                    Some(e)
                } else {
                    None
                }
            }
        })
        .unwrap_or_else(|| body.chars().take(240).collect());
    match code {
        400 | 422 | 428 => {
            Status::invalid_argument(format!("AskMe rejected request ({code}): {msg}"))
        }
        401 => Status::unauthenticated(format!("AskMe auth failed ({code}): {msg}")),
        403 => Status::permission_denied(format!("AskMe permission denied ({code}): {msg}")),
        404 => Status::not_found(format!("AskMe resource not found ({code}): {msg}")),
        409 => Status::aborted(format!("AskMe conflict ({code}): {msg}")),
        429 => Status::resource_exhausted(format!("AskMe overloaded ({code}): {msg}")),
        503 => Status::unavailable(format!("AskMe unavailable ({code}): {msg}")),
        504 => Status::deadline_exceeded(format!("AskMe timeout ({code}): {msg}")),
        _ => Status::internal(format!("AskMe HTTP {code}: {msg}")),
    }
}

fn extract_playback(val: &JsonVal, raw: String) -> doso::VoicePlayback {
    let message = {
        let cm = val.get_str("customer_message");
        if !cm.is_empty() {
            cm
        } else {
            val.get("error")
                .map(|e| e.get_str("message"))
                .unwrap_or_default()
        }
    };
    doso::VoicePlayback {
        playback_id: val.get_str("playback_id"),
        state: val.get_str("state"),
        priority: val.get_str("priority"),
        text_chars: val.get_f64("text_chars").max(0.0) as u32,
        message,
        raw_json: raw,
    }
}

pub async fn voice_status(
    cfg: &AskMeVoiceConfig,
) -> std::result::Result<doso::VoiceSystemStatus, Status> {
    let (code, sys_raw) = http_json(cfg, "GET", "/api/voice/system", None, &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &sys_raw));
    }
    let sys = parse_json(&sys_raw)
        .map_err(|e| Status::internal(format!("invalid AskMe /api/voice/system JSON: {e}")))?;
    let active_profile = match http_json(cfg, "GET", "/api/voice/profiles", None, &[]).await {
        Ok((c, p_raw)) if (200..300).contains(&c) => parse_json(&p_raw)
            .ok()
            .map(|p| p.get_str("active_profile"))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let runtime = sys.get("runtime");
    let llm = runtime.and_then(|r| r.get("llm"));
    let asr = runtime.and_then(|r| r.get("asr"));
    let tts = runtime.and_then(|r| r.get("tts"));

    Ok(doso::VoiceSystemStatus {
        status: sys.get_str("status"),
        service: "askme".to_owned(),
        active_profile,
        llm_provider: llm.map(|x| x.get_str("provider")).unwrap_or_default(),
        llm_model: llm.map(|x| x.get_str("model")).unwrap_or_default(),
        asr_provider: asr.map(|x| x.get_str("provider")).unwrap_or_default(),
        tts_provider: tts
            .map(|x| {
                let p = x.get_str("provider");
                if !p.is_empty() {
                    p
                } else {
                    x.get_str("backend")
                }
            })
            .unwrap_or_default(),
        raw_json: sys_raw,
    })
}

pub async fn voice_list_profiles(
    cfg: &AskMeVoiceConfig,
) -> std::result::Result<doso::VoiceProfileList, Status> {
    let (code, raw) = http_json(cfg, "GET", "/api/voice/profiles", None, &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val = parse_json(&raw)
        .map_err(|e| Status::internal(format!("invalid AskMe /api/voice/profiles JSON: {e}")))?;
    let profiles = val
        .get("profiles")
        .and_then(JsonVal::as_arr)
        .map(|items| {
            items
                .iter()
                .map(|p| doso::VoiceProfileInfo {
                    profile_id: p.get_str("profile_id"),
                    label: p.get_str("label"),
                    voice_id: p.get_str("voice_id"),
                    category: p.get_str("category"),
                    speed: p.get_f64("speed"),
                    volume: p.get_f64("volume"),
                    pitch: p.get_f64("pitch"),
                    emotion: p.get_str("emotion"),
                })
                .collect()
        })
        .unwrap_or_default();
    let available_sound_cues = val
        .get("available_sound_cues")
        .and_then(JsonVal::as_arr)
        .map(|arr| {
            arr.iter()
                .filter_map(JsonVal::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(doso::VoiceProfileList {
        active_profile: val.get_str("active_profile"),
        profiles,
        sound_cues_enabled: val.get_bool("sound_cues_enabled"),
        available_sound_cues,
        raw_json: raw,
    })
}

pub async fn voice_switch_profile(
    cfg: &AskMeVoiceConfig,
    request_id: &str,
    profile_id: &str,
) -> std::result::Result<doso::Receipt, Status> {
    let payload = format!("{{\"profile_id\":{}}}", json_escape(profile_id));
    let (code, raw) = http_json(cfg, "POST", "/api/voice/profile", Some(&payload), &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val = parse_json(&raw).unwrap_or(JsonVal::Null);
    let updated = val.get_bool("updated");
    if !updated {
        let reason = val.get_str("reason");
        return Err(Status::failed_precondition(if reason.is_empty() {
            "voice profile update was not applied".into()
        } else {
            reason
        }));
    }
    Ok(doso::Receipt {
        ok: true,
        accepted: Some(true),
        request_id: request_id.to_owned(),
        stage: "voice_profile_switched".to_owned(),
        ..Default::default()
    })
}

pub async fn voice_speak(
    cfg: &AskMeVoiceConfig,
    request_id: &str,
    req: &doso::VoiceSpeakRequest,
) -> std::result::Result<doso::VoicePlayback, Status> {
    let priority = if req.priority.trim().is_empty() {
        "normal"
    } else {
        req.priority.trim()
    };
    let queue_policy = if req.queue_policy.trim().is_empty() {
        "enqueue"
    } else {
        req.queue_policy.trim()
    };
    let mut fields = vec![
        format!("\"text\":{}", json_escape(&req.text)),
        format!("\"robot_id\":{}", json_escape(&cfg.robot_id)),
        format!("\"device_id\":{}", json_escape(&cfg.robot_id)),
        "\"semantics\":\"verbatim\"".to_owned(),
        format!("\"priority\":{}", json_escape(priority)),
        format!("\"queue_policy\":{}", json_escape(queue_policy)),
    ];
    if !req.voice_profile_id.trim().is_empty() {
        fields.push(format!(
            "\"voice_profile_id\":{}",
            json_escape(req.voice_profile_id.trim())
        ));
    }
    if let Some(speed) = req.speed {
        fields.push(format!("\"speed\":{speed}"));
    }
    if let Some(pitch) = req.pitch {
        fields.push(format!("\"pitch\":{pitch}"));
    }
    if let Some(volume) = req.volume {
        fields.push(format!("\"volume\":{volume}"));
    }
    let payload = format!("{{{}}}", fields.join(","));
    let (code, raw) = http_json(
        cfg,
        "POST",
        "/api/voice/speak",
        Some(&payload),
        &[("Idempotency-Key", request_id)],
    )
    .await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val = parse_json(&raw)
        .map_err(|e| Status::internal(format!("invalid AskMe /api/voice/speak JSON: {e}")))?;
    Ok(extract_playback(&val, raw))
}

pub async fn voice_playback_status(
    cfg: &AskMeVoiceConfig,
    playback_id: &str,
) -> std::result::Result<doso::VoicePlayback, Status> {
    let path = format!("/api/voice/playbacks/{playback_id}");
    let (code, raw) = http_json(cfg, "GET", &path, None, &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val = parse_json(&raw)
        .map_err(|e| Status::internal(format!("invalid AskMe playback status JSON: {e}")))?;
    Ok(extract_playback(&val, raw))
}

pub async fn voice_cancel_playback(
    cfg: &AskMeVoiceConfig,
    playback_id: &str,
    reason: &str,
) -> std::result::Result<doso::VoicePlayback, Status> {
    let path = format!("/api/voice/playbacks/{playback_id}/cancel");
    let reason = if reason.trim().is_empty() {
        "operator_cancelled"
    } else {
        reason.trim()
    };
    let payload = format!("{{\"reason\":{}}}", json_escape(reason));
    let (code, raw) = http_json(cfg, "POST", &path, Some(&payload), &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val = parse_json(&raw)
        .map_err(|e| Status::internal(format!("invalid AskMe cancel playback JSON: {e}")))?;
    Ok(extract_playback(&val, raw))
}

pub async fn voice_switch_component(
    cfg: &AskMeVoiceConfig,
    request_id: &str,
    req: &doso::VoiceSwitchComponentRequest,
) -> std::result::Result<doso::Receipt, Status> {
    let mut fields = vec![
        format!("\"component\":{}", json_escape(req.component.trim())),
        format!("\"provider\":{}", json_escape(req.provider.trim())),
        format!(
            "\"validate\":{}",
            if req.validate { "true" } else { "false" }
        ),
    ];
    if !req.model.trim().is_empty() {
        fields.push(format!("\"model\":{}", json_escape(req.model.trim())));
    }
    let payload = format!("{{{}}}", fields.join(","));
    let (code, raw) =
        http_json(cfg, "POST", "/api/voice/system/switch", Some(&payload), &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    Ok(doso::Receipt {
        ok: true,
        accepted: Some(true),
        request_id: request_id.to_owned(),
        stage: "voice_component_switched".to_owned(),
        ..Default::default()
    })
}

pub async fn voice_update_prompt(
    cfg: &AskMeVoiceConfig,
    request_id: &str,
    system_prompt: &str,
) -> std::result::Result<doso::Receipt, Status> {
    let payload = format!("{{\"system_prompt\":{}}}", json_escape(system_prompt));
    let (code, raw) =
        http_json(cfg, "POST", "/api/voice/system/prompt", Some(&payload), &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    Ok(doso::Receipt {
        ok: true,
        accepted: Some(true),
        request_id: request_id.to_owned(),
        stage: "voice_prompt_updated".to_owned(),
        ..Default::default()
    })
}

pub async fn voice_chat(
    cfg: &AskMeVoiceConfig,
    req: &doso::VoiceChatRequest,
) -> std::result::Result<doso::VoiceChatReply, Status> {
    let mut fields = vec![format!("\"text\":{}", json_escape(&req.text))];
    if !req.thread_id.trim().is_empty() {
        fields.push(format!(
            "\"conversation_thread_id\":{}",
            json_escape(req.thread_id.trim())
        ));
    }
    if req.speak_reply {
        fields.push("\"speak\":true".to_owned());
    }
    let payload = format!("{{{}}}", fields.join(","));
    let path = if req.speak_reply {
        "/api/runtime/voice-turn"
    } else {
        "/api/chat"
    };
    let (code, raw) = http_json(cfg, "POST", path, Some(&payload), &[]).await?;
    if !(200..300).contains(&code) {
        return Err(map_http_error(code, &raw));
    }
    let val =
        parse_json(&raw).map_err(|e| Status::internal(format!("invalid AskMe chat JSON: {e}")))?;
    let reply_text = {
        let r = val.get_str("reply");
        if !r.is_empty() {
            r
        } else {
            let resp = val.get_str("response");
            if !resp.is_empty() {
                resp
            } else {
                val.get_str("message")
            }
        }
    };
    let thread_id = {
        let t = val.get_str("conversation_thread_id");
        if !t.is_empty() {
            t
        } else {
            val.get_str("thread_id")
        }
    };
    Ok(doso::VoiceChatReply {
        reply_text,
        thread_id,
        turn_id: val.get_str("turn_id"),
        latency_ms: val.get_f64("latency_ms"),
        raw_json: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_gateway::{
        spawn_base_gateway_on_listener, BaseGatewayConfig, BaseGatewayService,
    };
    use doso_sdk::{RobotClient, VoiceSpeakOptions};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    #[test]
    fn json_round_trip_handles_chinese_and_escapes() {
        let raw = r#"{"status":"ready","runtime":{"llm":{"provider":"deepseek","model":"deepseek-v4-flash"}},"label":"\u5de1\u68c0\u64ad\u62a5","count":42,"ok":true}"#;
        let parsed = parse_json(raw).expect("valid JSON");
        assert_eq!(parsed.get_str("status"), "ready");
        assert_eq!(parsed.get_str("label"), "巡检播报");
        assert_eq!(parsed.get_f64("count"), 42.0);
        assert!(parsed.get_bool("ok"));
        assert_eq!(
            parsed
                .get("runtime")
                .and_then(|r| r.get("llm"))
                .map(|l| l.get_str("model"))
                .as_deref(),
            Some("deepseek-v4-flash")
        );
    }

    #[tokio::test]
    async fn gateway_voice_proxy_round_trips_askme_http_api() {
        let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_addr = http_listener.local_addr().unwrap();
        let http_task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = http_listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first = req.lines().next().unwrap_or("");
                    let (code, body) = if first.starts_with("GET /api/voice/system ") {
                        (
                            "200 OK",
                            r#"{"status":"ready","runtime":{"llm":{"provider":"deepseek","model":"deepseek-v4-flash"},"asr":{"provider":"cloud+local"},"tts":{"provider":"minimax"}}}"#,
                        )
                    } else if first.starts_with("GET /api/voice/profiles ") {
                        (
                            "200 OK",
                            r#"{"active_profile":"patrol_default","sound_cues_enabled":true,"available_sound_cues":["soft_chime"],"profiles":[{"profile_id":"patrol_default","label":"巡检播报","voice_id":"male-qn-qingse","category":"operations","speed":1.0,"volume":1.0,"pitch":0,"emotion":""}]}"#,
                        )
                    } else if first.starts_with("POST /api/voice/profile ") {
                        (
                            "200 OK",
                            r#"{"updated":true,"active_profile":"patrol_default"}"#,
                        )
                    } else if first.starts_with("POST /api/voice/speak ") {
                        assert!(req.contains("Idempotency-Key:"));
                        assert!(req.contains("X-Askme-Operator-Id: supervisor-1"));
                        (
                            "202 Accepted",
                            r#"{"playback_id":"pb-42","state":"queued","priority":"normal","text_chars":4,"customer_message":"queued"}"#,
                        )
                    } else if first.starts_with("GET /api/voice/playbacks/pb-42 ") {
                        (
                            "200 OK",
                            r#"{"playback_id":"pb-42","state":"completed","priority":"normal","text_chars":4,"customer_message":"done"}"#,
                        )
                    } else if first.starts_with("POST /api/voice/playbacks/pb-42/cancel ") {
                        (
                            "200 OK",
                            r#"{"playback_id":"pb-42","state":"cancelled","priority":"normal","text_chars":0,"customer_message":"cancelled"}"#,
                        )
                    } else if first.starts_with("POST /api/voice/system/switch ") {
                        (
                            "200 OK",
                            r#"{"updated":true,"component":"llm","state":"active"}"#,
                        )
                    } else if first.starts_with("POST /api/voice/system/prompt ") {
                        ("200 OK", r#"{"updated":true}"#)
                    } else if first.starts_with("POST /api/chat ") {
                        (
                            "200 OK",
                            r#"{"reply":"你好，我是小算","conversation_thread_id":"t-1","turn_id":"turn-9","latency_ms":123.4}"#,
                        )
                    } else {
                        ("404 Not Found", r#"{"error":"not_found"}"#)
                    };
                    let resp = format!(
                        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });

        let gw_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let cfg = BaseGatewayConfig {
            askme_voice: Some(AskMeVoiceConfig {
                endpoint: format!("http://{http_addr}"),
                operator_id: "supervisor-1".into(),
                api_token: None,
                robot_id: "xiaosuan-1".into(),
                timeout: Duration::from_secs(5),
            }),
            ..BaseGatewayConfig::default()
        };
        let (gw_addr, gw_handle) =
            spawn_base_gateway_on_listener(BaseGatewayService::new(cfg), gw_listener, stop_rx)
                .await
                .unwrap();

        let robot = RobotClient::connect(format!("http://{gw_addr}"))
            .await
            .unwrap();
        let caps = robot.system().capabilities().await.unwrap();
        let fields = caps.fields.unwrap().fields;
        assert_eq!(
            fields.get("voice").and_then(|v| v.kind.as_ref()),
            Some(&prost_types::value::Kind::BoolValue(true))
        );

        let st = robot.voice().status().await.unwrap();
        assert_eq!(st.service, "askme");
        assert_eq!(st.active_profile, "patrol_default");
        assert_eq!(st.llm_model, "deepseek-v4-flash");

        let profs = robot.voice().list_profiles().await.unwrap();
        assert_eq!(profs.active_profile, "patrol_default");
        assert_eq!(profs.profiles[0].label, "巡检播报");

        robot
            .voice()
            .switch_profile("patrol_default", None)
            .await
            .unwrap();
        let pb = robot
            .voice()
            .speak("测试播报", VoiceSpeakOptions::default(), None)
            .await
            .unwrap();
        assert_eq!(pb.playback_id, "pb-42");
        assert_eq!(
            robot.voice().playback_status("pb-42").await.unwrap().state,
            "completed"
        );
        assert_eq!(
            robot
                .voice()
                .cancel_playback("pb-42", "stop", None)
                .await
                .unwrap()
                .state,
            "cancelled"
        );
        robot
            .voice()
            .switch_component("llm", "deepseek", Some("deepseek-v4-flash"), false, None)
            .await
            .unwrap();
        robot
            .voice()
            .update_prompt("你是小算。", None)
            .await
            .unwrap();
        let reply = robot
            .voice()
            .chat("你好", Some("t-1"), false, None)
            .await
            .unwrap();
        assert_eq!(reply.reply_text, "你好，我是小算");
        assert_eq!(reply.thread_id, "t-1");

        let _ = stop_tx.send(());
        let _ = gw_handle.await;
        http_task.abort();
    }
}
