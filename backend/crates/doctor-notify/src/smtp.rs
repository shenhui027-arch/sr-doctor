//! A minimal SMTP client for one message to one mailbox: implicit TLS,
//! STARTTLS or plain; AUTH PLAIN or LOGIN when a username is set. Bodies go
//! base64-encoded as multipart/alternative text + HTML, so no line of the
//! payload can end the DATA section or inject a header.
use std::io::{Read, Write};
use std::net::TcpStream;

use doctor_contracts::{EmailSecurity, NotifyErrorClass};

use crate::message::Email;
use crate::transport::{self, TlsStream};
use crate::{SendError, base64, random_hex};

pub(crate) struct Server<'a> {
    pub host: &'a str,
    pub port: u16,
    pub security: EmailSecurity,
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub from_address: &'a str,
    pub from_name: Option<&'a str>,
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream>),
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

struct Session {
    stream: Stream,
    pending: Vec<u8>,
}

const MAX_REPLY_BYTES: usize = 64 * 1024;

impl Session {
    fn line(&mut self) -> Result<String, SendError> {
        loop {
            if let Some(end) = self.pending.windows(2).position(|window| window == b"\r\n") {
                let line: Vec<u8> = self.pending.drain(..end + 2).take(end).collect();
                return String::from_utf8(line)
                    .map_err(|_| SendError::new(NotifyErrorClass::InvalidResponse));
            }
            if self.pending.len() > MAX_REPLY_BYTES {
                return Err(SendError::new(NotifyErrorClass::InvalidResponse));
            }
            let mut chunk = [0u8; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(SendError::new(NotifyErrorClass::Network)),
                Ok(read) => self.pending.extend_from_slice(&chunk[..read]),
                Err(error) => return Err(transport::io_error(&error)),
            }
        }
    }

    /// A whole (possibly multi-line) reply: its code and the text lines.
    fn reply(&mut self) -> Result<(u16, Vec<String>), SendError> {
        let mut lines = Vec::new();
        loop {
            let line = self.line()?;
            let code = line
                .get(..3)
                .and_then(|code| code.parse::<u16>().ok())
                .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_owned());
            if !more {
                return Ok((code, lines));
            }
        }
    }

    fn send(&mut self, command: &str) -> Result<(), SendError> {
        self.stream
            .write_all(command.as_bytes())
            .and_then(|()| self.stream.write_all(b"\r\n"))
            .and_then(|()| self.stream.flush())
            .map_err(|error| transport::io_error(&error))
    }

    fn expect(&mut self, accepted: &[u16]) -> Result<Vec<String>, SendError> {
        let (code, lines) = self.reply()?;
        if accepted.contains(&code) {
            Ok(lines)
        } else {
            Err(reply_error(code))
        }
    }

    fn command(&mut self, command: &str, accepted: &[u16]) -> Result<Vec<String>, SendError> {
        self.send(command)?;
        self.expect(accepted)
    }
}

fn reply_error(code: u16) -> SendError {
    let class = match code {
        530 | 534 | 535 | 454 => NotifyErrorClass::AuthFailed,
        _ => NotifyErrorClass::Rejected,
    };
    SendError::coded(class, code)
}

fn encoded_word(text: &str) -> String {
    if text.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return text.to_owned();
    }
    // At most 45 raw bytes per word keeps each encoded word under 75 chars.
    let mut words = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if current.len() + c.len_utf8() > 45 {
            words.push(format!("=?UTF-8?B?{}?=", base64(current.as_bytes())));
            current.clear();
        }
        current.push(c);
    }
    if !current.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", base64(current.as_bytes())));
    }
    words.join("\r\n ")
}

fn wrapped_base64(text: &str) -> String {
    let encoded = base64(text.as_bytes());
    encoded
        .as_bytes()
        .chunks(76)
        .map(|line| std::str::from_utf8(line).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub(crate) fn message_bytes(server: &Server<'_>, to: &str, mail: &Email, now: i64) -> String {
    let date = time::OffsetDateTime::from_unix_timestamp(now)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc2822)
                .ok()
        })
        .unwrap_or_default();
    let boundary = format!("sr-doctor-{}", random_hex(12));
    let from = match server.from_name.filter(|name| !name.is_empty()) {
        Some(name) => format!("{} <{}>", encoded_word(name), server.from_address),
        None => format!("<{}>", server.from_address),
    };
    let domain = server
        .from_address
        .rsplit_once('@')
        .map_or("sr-doctor", |(_, domain)| domain);
    format!(
        "From: {from}\r\nTo: <{to}>\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: <{}@{domain}>\r\nMIME-Version: 1.0\r\nAuto-Submitted: auto-generated\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--{boundary}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--{boundary}--\r\n",
        encoded_word(&mail.subject),
        random_hex(16),
        wrapped_base64(&mail.text),
        wrapped_base64(&mail.html),
    )
}

fn ehlo(session: &mut Session) -> Result<Vec<String>, SendError> {
    session.command("EHLO sr-doctor", &[250])
}

fn advertises(capabilities: &[String], keyword: &str) -> bool {
    capabilities.iter().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
    })
}

fn auth_methods(capabilities: &[String]) -> Vec<String> {
    capabilities
        .iter()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            words
                .next()
                .filter(|word| word.eq_ignore_ascii_case("AUTH"))
                .map(|_| words.map(str::to_ascii_uppercase).collect::<Vec<_>>())
        })
        .flatten()
        .collect()
}

pub(crate) fn send(server: &Server<'_>, to: &str, mail: &Email, now: i64) -> Result<(), SendError> {
    let tcp = transport::connect_tcp(server.host, server.port)?;
    let stream = match server.security {
        EmailSecurity::Tls => Stream::Tls(Box::new(transport::tls_connect(tcp, server.host)?)),
        EmailSecurity::Starttls | EmailSecurity::Plain => Stream::Plain(tcp),
    };
    let mut session = Session {
        stream,
        pending: Vec::new(),
    };
    session.expect(&[220])?;
    let mut capabilities = ehlo(&mut session)?;
    if server.security == EmailSecurity::Starttls {
        if !advertises(&capabilities, "STARTTLS") {
            return Err(SendError::new(NotifyErrorClass::Tls));
        }
        session.command("STARTTLS", &[220])?;
        // Anything already buffered would be unauthenticated plaintext.
        if !session.pending.is_empty() {
            return Err(SendError::new(NotifyErrorClass::InvalidResponse));
        }
        let Stream::Plain(tcp) = session.stream else {
            return Err(SendError::new(NotifyErrorClass::Tls));
        };
        session.stream = Stream::Tls(Box::new(transport::tls_connect(tcp, server.host)?));
        capabilities = ehlo(&mut session)?;
    }
    if let Some(username) = server.username {
        let password = server
            .password
            .ok_or_else(|| SendError::new(NotifyErrorClass::NotConfigured))?;
        let methods = auth_methods(&capabilities);
        if methods.iter().any(|method| method == "PLAIN") {
            let token = base64(format!("\0{username}\0{password}").as_bytes());
            session.command(&format!("AUTH PLAIN {token}"), &[235])?;
        } else if methods.iter().any(|method| method == "LOGIN") {
            session.command("AUTH LOGIN", &[334])?;
            session.command(&base64(username.as_bytes()), &[334])?;
            session.command(&base64(password.as_bytes()), &[235])?;
        } else {
            return Err(SendError::new(NotifyErrorClass::AuthFailed));
        }
    }
    session.command(&format!("MAIL FROM:<{}>", server.from_address), &[250])?;
    session.command(&format!("RCPT TO:<{to}>"), &[250, 251])?;
    session.command("DATA", &[354])?;
    let body = message_bytes(server, to, mail, now);
    session
        .stream
        .write_all(body.as_bytes())
        .and_then(|()| session.stream.write_all(b".\r\n"))
        .and_then(|()| session.stream.flush())
        .map_err(|error| transport::io_error(&error))?;
    session.expect(&[250])?;
    // The message is accepted; a failed QUIT changes nothing.
    let _ = session.command("QUIT", &[221]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn server(port: u16, username: Option<&'static str>) -> Server<'static> {
        Server {
            host: "127.0.0.1",
            port,
            security: EmailSecurity::Plain,
            username,
            password: Some("p@ss"),
            from_address: "alerts@example.com",
            from_name: Some("SR Doctor 告警"),
        }
    }

    fn mail() -> Email {
        Email {
            subject: "[SR Doctor] 2 条新告警".into(),
            text: "第一行\n.\n第二行".into(),
            html: "<p>x</p>".into(),
        }
    }

    /// A scripted SMTP peer: answers each line with the next reply and
    /// returns what the client sent.
    fn peer(auth: &'static str, fail_auth: bool) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            let mut seen = Vec::new();
            writer.write_all(b"220 mx ready\r\n").unwrap();
            let mut in_data = false;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                let text = line.trim_end_matches("\r\n").to_owned();
                line.clear();
                if in_data {
                    if text == "." {
                        in_data = false;
                        writer.write_all(b"250 queued\r\n").unwrap();
                    }
                    seen.push(text);
                    continue;
                }
                seen.push(text.clone());
                let reply: String = if text.starts_with("EHLO") {
                    format!("250-mx\r\n250-SIZE 1000000\r\n250 AUTH {auth}\r\n")
                } else if text.starts_with("AUTH PLAIN") {
                    if fail_auth {
                        "535 5.7.8 bad credentials\r\n".into()
                    } else {
                        "235 ok\r\n".into()
                    }
                } else if text == "AUTH LOGIN" {
                    "334 VXNlcm5hbWU6\r\n".into()
                } else if text == base64(b"alerts@example.com") {
                    "334 UGFzc3dvcmQ6\r\n".into()
                } else if text == base64(b"p@ss") {
                    "235 ok\r\n".into()
                } else if text.starts_with("MAIL FROM") || text.starts_with("RCPT TO") {
                    "250 ok\r\n".into()
                } else if text == "DATA" {
                    in_data = true;
                    "354 go\r\n".into()
                } else if text == "QUIT" {
                    writer.write_all(b"221 bye\r\n").unwrap();
                    break;
                } else {
                    "500 what\r\n".into()
                };
                writer.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (port, handle)
    }

    #[test]
    fn n02_plain_auth_dialogue_sends_one_base64_message() {
        let (port, handle) = peer("PLAIN LOGIN", false);
        send(
            &server(port, Some("alerts@example.com")),
            "dba@example.com",
            &mail(),
            1_793_264_400,
        )
        .unwrap();
        let seen = handle.join().unwrap();
        assert_eq!(seen[0], "EHLO sr-doctor");
        assert_eq!(
            seen[1],
            format!("AUTH PLAIN {}", base64(b"\0alerts@example.com\0p@ss"))
        );
        assert_eq!(seen[2], "MAIL FROM:<alerts@example.com>");
        assert_eq!(seen[3], "RCPT TO:<dba@example.com>");
        assert_eq!(seen[4], "DATA");
        let data = seen[5..].join("\n");
        assert!(data.contains("Subject: =?UTF-8?B?"), "{data}");
        assert!(data.contains("To: <dba@example.com>"));
        assert!(data.contains("Content-Transfer-Encoding: base64"));
        // The lone "." of the text body is inside base64, never a raw line.
        assert_eq!(seen.iter().filter(|line| *line == ".").count(), 1);
        assert_eq!(seen.last().unwrap(), "QUIT");
    }

    #[test]
    fn n02_login_auth_when_plain_is_not_offered() {
        let (port, handle) = peer("LOGIN", false);
        send(
            &server(port, Some("alerts@example.com")),
            "dba@example.com",
            &mail(),
            0,
        )
        .unwrap();
        let seen = handle.join().unwrap();
        assert_eq!(seen[1], "AUTH LOGIN");
    }

    #[test]
    fn n02_rejected_credentials_are_auth_failed_with_the_code() {
        let (port, handle) = peer("PLAIN", true);
        let error = send(
            &server(port, Some("alerts@example.com")),
            "dba@example.com",
            &mail(),
            0,
        )
        .unwrap_err();
        assert_eq!(
            error,
            SendError {
                class: NotifyErrorClass::AuthFailed,
                provider_code: Some("535".into())
            }
        );
        drop(handle);
    }

    #[test]
    fn n02_no_username_skips_auth() {
        let (port, handle) = peer("PLAIN", false);
        send(&server(port, None), "dba@example.com", &mail(), 0).unwrap();
        assert_eq!(handle.join().unwrap()[1], "MAIL FROM:<alerts@example.com>");
    }

    #[test]
    fn n02_headers_encode_non_ascii_and_stay_short() {
        let long = "告警".repeat(40);
        let word = encoded_word(&long);
        assert!(
            word.split("\r\n ")
                .all(|part| part.len() <= 75 && part.starts_with("=?UTF-8?B?"))
        );
        assert_eq!(encoded_word("plain ascii"), "plain ascii");
        let bytes = message_bytes(&server(25, None), "dba@example.com", &mail(), 1_793_264_400);
        assert!(
            bytes.contains("Date: Thu, 29 Oct 2026 09:00:00 +0000"),
            "{bytes}"
        );
        assert!(bytes.starts_with("From: =?UTF-8?B?"));
    }
}
