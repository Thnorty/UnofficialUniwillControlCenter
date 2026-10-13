//! The plugins' address on this PC: `http://127.0.0.1:47823/plugins/<id>[/more]`. Anything POSTed there as JSON
//! goes to that plugin's `on_event(path, body)`, which is how Claude Code's hooks and the browser extension
//! reach their plugins.
//!
//! Only this PC can connect. Web pages can't post here: requests from a page carry its Origin and are refused, and
//! the JSON content type makes the browser ask first (a preflight), which gets no permission. Browser extensions
//! with permission for this address are let through.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use super::PORT;

const MAX_HEADERS: usize = 64 << 10;
const MAX_BODY: usize = 1 << 20;

/// Starts listening. `deliver(id, path, body)` gets each event.
pub fn start(deliver: impl Fn(&str, &str, Value) + Send + 'static) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", PORT))?;
    std::thread::Builder::new().name("Plugin server".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
            handle(stream, &deliver);
        }
    })?;
    Ok(())
}

fn handle(mut stream: TcpStream, deliver: &impl Fn(&str, &str, Value)) {
    let status = match read_request(&stream) {
        Ok(request) => respond(request, deliver),
        Err(status) => status,
    };
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Unsupported Media Type",
    };
    let _ = write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.flush();
}

struct Request {
    method: String,
    path: String,
    origin: Option<String>,
    content_type: String,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Result<Request, u16> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut read_line = |line: &mut String, total: &mut usize| -> Result<(), u16> {
        line.clear();
        let n = reader.by_ref().take((MAX_HEADERS - *total) as u64).read_line(line).map_err(|_| 400u16)?;
        *total += n;
        if n == 0 || *total >= MAX_HEADERS {
            return Err(400);
        }
        Ok(())
    };
    let mut total = 0;
    read_line(&mut line, &mut total)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut length, mut origin, mut content_type) = (0usize, None, String::new());
    loop {
        read_line(&mut line, &mut total)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().map_err(|_| 400u16)?,
            "origin" => origin = Some(value.to_string()),
            "content-type" => content_type = value.to_ascii_lowercase(),
            "transfer-encoding" => return Err(400), // chunked bodies aren't needed
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok(Request { method, path, origin, content_type, body })
}

fn respond(request: Request, deliver: &impl Fn(&str, &str, Value)) -> u16 {
    if let Some(origin) = &request.origin
        && !(origin.starts_with("chrome-extension://") || origin.starts_with("moz-extension://"))
    {
        return 403; // a web page
    }
    let path = request.path.split('?').next().unwrap_or("");
    match request.method.as_str() {
        "GET" if path == "/" => 200,
        "OPTIONS" => 204, // without CORS headers, so web pages' preflights fail
        "POST" => {
            let Some(rest) = path.strip_prefix("/plugins/") else { return 404 };
            let (id, sub) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, ""),
            };
            if id.is_empty() {
                return 404;
            }
            if !request.content_type.starts_with("application/json") {
                return 415;
            }
            let body = if request.body.is_empty() {
                Value::Null
            } else {
                match serde_json::from_slice(&request.body) {
                    Ok(value) => value,
                    Err(_) => return 400,
                }
            };
            deliver(id, sub, body);
            200 // also for plugins that are off, so Claude Code doesn't report failed hooks
        }
        _ => 405,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, path: &str, origin: Option<&str>, content_type: &str, body: &str) -> Request {
        Request { method: method.into(), path: path.into(), origin: origin.map(String::from), content_type: content_type.into(), body: body.as_bytes().to_vec() }
    }

    #[test]
    fn routes_and_refuses_web_pages() {
        let got = std::cell::RefCell::new(Vec::new());
        let deliver = |id: &str, sub: &str, body: Value| got.borrow_mut().push((id.to_string(), sub.to_string(), body));
        assert_eq!(respond(request("POST", "/plugins/claude-code", None, "application/json", r#"{"a":1}"#), &deliver), 200);
        assert_eq!(respond(request("POST", "/plugins/downloads/progress", Some("chrome-extension://abc"), "application/json", "{}"), &deliver), 200);
        assert_eq!(respond(request("POST", "/plugins/x", Some("https://evil.example"), "application/json", "{}"), &deliver), 403);
        assert_eq!(respond(request("POST", "/plugins/x", None, "text/plain", "{}"), &deliver), 415);
        assert_eq!(respond(request("POST", "/other", None, "application/json", "{}"), &deliver), 404);
        let got = got.into_inner();
        assert_eq!(got.len(), 2);
        assert_eq!((got[1].0.as_str(), got[1].1.as_str()), ("downloads", "/progress"));
        assert_eq!(got[0].2["a"], 1);
    }
}
