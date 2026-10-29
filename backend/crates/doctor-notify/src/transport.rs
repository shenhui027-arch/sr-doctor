//! Blocking network primitives for one send: TCP with fixed timeouts, TLS
//! that always verifies against the system roots, and a minimal HTTPS POST.
//! Callers run these on a blocking thread, one send at a time.
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use doctor_contracts::NotifyErrorClass;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};

use crate::SendError;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) fn io_error(error: &std::io::Error) -> SendError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            SendError::new(NotifyErrorClass::Timeout)
        }
        // rustls reports handshake and certificate failures as InvalidData.
        std::io::ErrorKind::InvalidData => SendError::new(NotifyErrorClass::Tls),
        _ => SendError::new(NotifyErrorClass::Network),
    }
}

pub(crate) fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, SendError> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| SendError::new(NotifyErrorClass::Network))?;
    let mut last = SendError::new(NotifyErrorClass::Network);
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                let configure = stream
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
                    .and_then(|()| stream.set_nodelay(true));
                configure.map_err(|error| io_error(&error))?;
                return Ok(stream);
            }
            Err(error) => last = io_error(&error),
        }
    }
    Err(last)
}

fn tls_config() -> Result<Arc<ClientConfig>, SendError> {
    static CONFIG: OnceLock<Option<Arc<ClientConfig>>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let loaded = rustls_native_certs::load_native_certs();
            let mut roots = RootCertStore::empty();
            for certificate in loaded.certs {
                let _ = roots.add(certificate);
            }
            if roots.is_empty() {
                return None;
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .ok()
                .map(|builder| {
                    Arc::new(builder.with_root_certificates(roots).with_no_client_auth())
                })
        })
        .clone()
        .ok_or_else(|| SendError::new(NotifyErrorClass::Tls))
}

pub(crate) type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Full verification against the system roots; there is no other mode.
pub(crate) fn tls_connect(tcp: TcpStream, host: &str) -> Result<TlsStream, SendError> {
    let server_name =
        ServerName::try_from(host.to_owned()).map_err(|_| SendError::new(NotifyErrorClass::Tls))?;
    let connection = ClientConnection::new(tls_config()?, server_name)
        .map_err(|_| SendError::new(NotifyErrorClass::Tls))?;
    let mut stream = StreamOwned::new(connection, tcp);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|error| match io_error(&error).class {
            NotifyErrorClass::Timeout => SendError::new(NotifyErrorClass::Timeout),
            _ => SendError::new(NotifyErrorClass::Tls),
        })?;
    if stream.conn.is_handshaking() {
        return Err(SendError::new(NotifyErrorClass::Tls));
    }
    Ok(stream)
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub(crate) fn request_bytes(
    host: &str,
    path_and_query: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut head = format!(
        "POST {path_and_query} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: sr-doctor/{}\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        env!("CARGO_PKG_VERSION"),
        body.len()
    );
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// `None` while the response is incomplete.
pub(crate) fn parse_response(buffer: &[u8]) -> Option<Result<HttpResponse, SendError>> {
    let invalid = || Some(Err(SendError::new(NotifyErrorClass::InvalidResponse)));
    let end = buffer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let Ok(head) = std::str::from_utf8(&buffer[..end]) else {
        return invalid();
    };
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return invalid();
    };
    let Some(status) = version
        .starts_with("HTTP/1.")
        .then(|| code.parse::<u16>().ok())
        .flatten()
    else {
        return invalid();
    };
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            match value.parse::<usize>() {
                Ok(parsed) => length = Some(parsed),
                Err(_) => return invalid(),
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    let rest = &buffer[end + 4..];
    if chunked {
        return match dechunk(rest) {
            Chunks::Complete(body) => Some(Ok(HttpResponse { status, body })),
            Chunks::Incomplete => None,
            Chunks::Invalid => invalid(),
        };
    }
    match length {
        Some(length) if rest.len() >= length => Some(Ok(HttpResponse {
            status,
            body: rest[..length].to_vec(),
        })),
        Some(_) => None,
        // No length: the body runs to the end of the connection.
        None => None,
    }
}

enum Chunks {
    Complete(Vec<u8>),
    Incomplete,
    Invalid,
}

fn dechunk(mut rest: &[u8]) -> Chunks {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return Chunks::Incomplete;
        };
        let Some(size) = std::str::from_utf8(&rest[..line_end])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|size| usize::from_str_radix(size.trim(), 16).ok())
        else {
            return Chunks::Invalid;
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Chunks::Complete(body);
        }
        if rest.len() < size + 2 {
            return Chunks::Incomplete;
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

/// One HTTPS POST with `Connection: close`. Only the status and a bounded
/// body come back; the caller keeps neither beyond a provider code.
pub(crate) fn https_post(
    host: &str,
    path_and_query: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Result<HttpResponse, SendError> {
    let tcp = connect_tcp(host, 443)?;
    let mut stream = tls_connect(tcp, host)?;
    stream
        .write_all(&request_bytes(host, path_and_query, headers, body))
        .and_then(|()| stream.flush())
        .map_err(|error| io_error(&error))?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(parsed) = parse_response(&buffer) {
            return parsed;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > MAX_RESPONSE_BYTES {
                    return Err(SendError::new(NotifyErrorClass::InvalidResponse));
                }
            }
            // Servers often close without close_notify once the body is sent.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(io_error(&error)),
        }
    }
    // End of stream: a body without length or chunking ends here.
    let end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
    match parse_response(&buffer) {
        Some(parsed) => parsed,
        None => {
            let status = std::str::from_utf8(&buffer[..end])
                .ok()
                .and_then(|head| head.split(' ').nth(1))
                .and_then(|code| code.parse().ok())
                .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
            Ok(HttpResponse {
                status,
                body: buffer[end + 4..].to_vec(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_one_post_with_a_length_and_close() {
        let bytes = request_bytes(
            "open.feishu.cn",
            "/open-apis/bot/v2/hook/abc",
            &[("Content-Type", "application/json".into())],
            b"{}",
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.starts_with(
                "POST /open-apis/bot/v2/hook/abc HTTP/1.1\r\nHost: open.feishu.cn\r\n"
            )
        );
        assert!(text.contains("Content-Length: 2\r\nConnection: close\r\n"));
        assert!(text.ends_with("Content-Type: application/json\r\n\r\n{}"));
    }

    #[test]
    fn responses_parse_by_length_or_chunks() {
        let fixed = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"code\":0}\n";
        assert_eq!(
            parse_response(fixed).unwrap().unwrap(),
            HttpResponse {
                status: 200,
                body: b"{\"code\":0}\n".to_vec()
            }
        );
        assert!(parse_response(&fixed[..fixed.len() - 3]).is_none());
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2;x=y\r\n1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked).unwrap().unwrap().body, b"{\"a\":1}");
        assert!(parse_response(&chunked[..60]).is_none());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
                .unwrap()
                .is_err()
        );
        assert!(parse_response(b"SMTP 220 hi\r\n\r\n").unwrap().is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n")
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn io_errors_map_to_classes() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            io_error(&Error::from(ErrorKind::TimedOut)).class,
            NotifyErrorClass::Timeout
        );
        assert_eq!(
            io_error(&Error::from(ErrorKind::InvalidData)).class,
            NotifyErrorClass::Tls
        );
        assert_eq!(
            io_error(&Error::from(ErrorKind::ConnectionRefused)).class,
            NotifyErrorClass::Network
        );
    }
}
