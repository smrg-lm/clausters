//! WebSocket **client** transport over the C ABI.
//!
//! The browser reaches a `--ws` server with its native `WebSocket`; a
//! non-browser binding (Python `ctypes`, JS N-API, ...) reaches it through these
//! five calls instead of re-implementing the WebSocket handshake and framing.
//! The protocol lives once, in `tungstenite` — the same crate the server's
//! `osc::ws` listener uses — so there is no second implementation to maintain.
//! Always compiled into the ffi cdylib (not feature-gated). `ws://` only; TLS
//! (`wss://`) is out of scope, terminate it at a reverse proxy.
//!
//! Stateful, like the embed server's handle: `clausters_ws_connect` returns an
//! opaque handle, `clausters_ws_send`/`clausters_ws_recv` move whole OSC packets
//! (one binary message each), `clausters_ws_close` frees it. A failing call
//! records a thread-local message readable with `clausters_ws_last_error`. Only
//! flat data crosses the boundary (byte buffers, integers, an error string),
//! per the project's boundary rule.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// An open client connection. Opaque to callers (the handle is a pointer).
pub struct WsClient {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: impl Into<Vec<u8>>) {
    let c = CString::new(msg).unwrap_or_else(|_| CString::new("error").expect("no nul"));
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// # Safety
/// `p` is null or a valid NUL-terminated C string.
unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

/// The underlying `TcpStream` (we only open `ws://`, the `Plain` variant), for
/// setting timeouts.
fn tcp_of(ws: &WebSocket<MaybeTlsStream<TcpStream>>) -> Option<&TcpStream> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        _ => None,
    }
}

/// Opens a WebSocket client to `ws://host:port/path`. Returns a handle, or null
/// on failure (then `clausters_ws_last_error` has the reason). Free the handle
/// with `clausters_ws_close`.
///
/// # Safety
/// `host` and `path` are NUL-terminated C strings (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ws_connect(
    host: *const c_char,
    port: u16,
    path: *const c_char,
) -> *mut WsClient {
    let (Some(host), Some(path)) = (unsafe { cstr(host) }, unsafe { cstr(path) }) else {
        set_error("null host or path");
        return std::ptr::null_mut();
    };
    let url = format!("ws://{host}:{port}{path}");
    match tungstenite::connect(&url) {
        Ok((ws, _resp)) => {
            if let Some(tcp) = tcp_of(&ws) {
                let _ = tcp.set_nodelay(true);
            }
            Box::into_raw(Box::new(WsClient { ws }))
        }
        Err(e) => {
            set_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Sends `len` bytes as one binary WebSocket message. Returns 0 on success, -1
/// on a null handle, -2 on a send error (`clausters_ws_last_error`).
///
/// # Safety
/// `handle` is a live handle from `clausters_ws_connect`; `ptr`/`len` describe a
/// readable buffer (or `ptr` is null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ws_send(
    handle: *mut WsClient,
    ptr: *const u8,
    len: usize,
) -> i32 {
    let Some(client) = (unsafe { handle.as_mut() }) else {
        return -1;
    };
    let bytes = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
    };
    match client.ws.send(Message::Binary(bytes)) {
        Ok(()) => 0,
        Err(e) => {
            set_error(e.to_string());
            -2
        }
    }
}

/// Receives one binary message into `buf` (capacity `cap`), waiting up to
/// `timeout_ms`. Returns the byte length written (>= 0), 0 on timeout, -1 on a
/// null handle, -2 on close/error, -3 if the message is larger than `cap`.
///
/// # Safety
/// `handle` is live; `buf`/`cap` describe a writable buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ws_recv(
    handle: *mut WsClient,
    buf: *mut u8,
    cap: usize,
    timeout_ms: u32,
) -> isize {
    let Some(client) = (unsafe { handle.as_mut() }) else {
        return -1;
    };
    let timeout = Duration::from_millis(timeout_ms.max(1) as u64);
    if let Some(tcp) = tcp_of(&client.ws) {
        let _ = tcp.set_read_timeout(Some(timeout));
    }
    let deadline = Instant::now() + timeout;
    loop {
        match client.ws.read() {
            Ok(Message::Binary(data)) => {
                if data.len() > cap {
                    set_error(format!("message {} bytes exceeds buffer {cap}", data.len()));
                    return -3;
                }
                if !data.is_empty() && !buf.is_null() {
                    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len()) };
                }
                return data.len() as isize;
            }
            Ok(Message::Close(_)) => {
                set_error("connection closed");
                return -2;
            }
            // Text/Ping/Pong/raw: tungstenite answers pings itself; keep waiting
            // for data until the deadline.
            Ok(_) => {
                if Instant::now() >= deadline {
                    return 0;
                }
            }
            // A read timeout (no data within the window) surfaces as
            // would-block/timed-out: report a clean timeout.
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return 0;
            }
            Err(e) => {
                set_error(e.to_string());
                return -2;
            }
        }
    }
}

/// Closes and frees a handle. Null is a no-op.
///
/// # Safety
/// `handle` is null or a live handle from `clausters_ws_connect`, not used
/// again afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ws_close(handle: *mut WsClient) {
    if handle.is_null() {
        return;
    }
    let mut client = unsafe { Box::from_raw(handle) };
    let _ = client.ws.close(None);
    let _ = client.ws.flush();
}

/// The last error on this thread as a NUL-terminated C string (empty if none).
/// Valid until the next failing call on this thread; copy it out.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_ws_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// `connect` → `send` → `recv` → `close` round-trips through a real
    /// WebSocket, embedded nulls and all, against an inline echo server.
    #[test]
    fn connect_send_recv_close_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            loop {
                match ws.read() {
                    Ok(Message::Binary(b)) => {
                        ws.send(Message::Binary(b)).unwrap();
                        break;
                    }
                    Ok(_) => continue,
                    Err(_) => return,
                }
            }
            let _ = ws.read(); // wait for the client's close
        });

        let host = CString::new("127.0.0.1").unwrap();
        let path = CString::new("/").unwrap();
        let handle = unsafe { clausters_ws_connect(host.as_ptr(), port, path.as_ptr()) };
        assert!(!handle.is_null(), "connect failed");

        // A binary payload with embedded NULs, as OSC packets have.
        let payload: &[u8] = b"abc\0def\0/status";
        let rc = unsafe { clausters_ws_send(handle, payload.as_ptr(), payload.len()) };
        assert_eq!(rc, 0);

        let mut buf = vec![0u8; 1024];
        let n = unsafe { clausters_ws_recv(handle, buf.as_mut_ptr(), buf.len(), 2000) };
        assert_eq!(n, payload.len() as isize);
        assert_eq!(&buf[..payload.len()], payload);

        unsafe { clausters_ws_close(handle) };
        let _ = server.join();
    }
}
