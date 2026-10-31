//! Outbound alert notifications: email over SMTP, Feishu and DingTalk custom
//! bots, and Alibaba Cloud / Tencent Cloud SMS. One blocking send per call;
//! the caller owns scheduling, retries and records. Nothing here logs, and
//! errors carry a category and at most a short provider code.
use doctor_contracts::{NotifyChannelConfig, NotifyErrorClass};

mod bots;
pub mod message;
pub mod secret;
mod sms;
mod smtp;
mod transport;
pub mod validate;

pub use message::{AlertBatch, AlertFact, Context, LoadAlertFact};
pub use secret::{KeyError, SecretKey};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendError {
    pub class: NotifyErrorClass,
    /// A provider's short error code (`19021`, `isv.BUSINESS_LIMIT_CONTROL`,
    /// SMTP `535`), never its message text.
    pub provider_code: Option<String>,
}

impl SendError {
    pub fn new(class: NotifyErrorClass) -> Self {
        Self {
            class,
            provider_code: None,
        }
    }

    pub(crate) fn coded(class: NotifyErrorClass, code: impl ToString) -> Self {
        Self {
            class,
            provider_code: provider_code(&code.to_string()),
        }
    }
}

/// Keeps only a short token of letters, digits and `._-`.
fn provider_code(code: &str) -> Option<String> {
    let code = code.trim();
    (!code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
    .then(|| code.to_owned())
}

/// Where one message goes. Bot webhooks and phone numbers come decrypted
/// from the store just for this send.
pub enum Recipient<'a> {
    Email(&'a str),
    Bot {
        webhook: &'a str,
        signing_secret: Option<&'a str>,
    },
    Phone(&'a str),
}

/// Sends one message. `channel_secret` is the SMTP password or the SMS
/// AccessKey Secret / SecretKey.
pub fn send(
    config: &NotifyChannelConfig,
    channel_secret: Option<&str>,
    recipient: &Recipient<'_>,
    batch: &AlertBatch,
    context: &Context<'_>,
    now: i64,
) -> Result<(), SendError> {
    let not_configured = || SendError::new(NotifyErrorClass::NotConfigured);
    match (config, recipient) {
        (
            NotifyChannelConfig::Email {
                host,
                port,
                security,
                username,
                from_address,
                from_name,
            },
            Recipient::Email(to),
        ) => {
            let mail = message::email(context, batch);
            smtp::send(
                &smtp::Server {
                    host,
                    port: *port,
                    security: *security,
                    username: username.as_deref().filter(|name| !name.is_empty()),
                    password: channel_secret,
                    from_address,
                    from_name: from_name.as_deref(),
                },
                to,
                &mail,
                now,
            )
        }
        (
            NotifyChannelConfig::Feishu {},
            Recipient::Bot {
                webhook,
                signing_secret,
            },
        ) => bots::feishu(webhook, *signing_secret, context, batch, now),
        (
            NotifyChannelConfig::Dingtalk {},
            Recipient::Bot {
                webhook,
                signing_secret,
            },
        ) => bots::dingtalk(webhook, *signing_secret, context, batch, now),
        (
            NotifyChannelConfig::AliyunSms {
                access_key_id,
                sign_name,
                template_code,
                region,
                ..
            },
            Recipient::Phone(phone),
        ) => {
            let secret = channel_secret.ok_or_else(not_configured)?;
            sms::aliyun(
                &sms::Aliyun {
                    access_key_id,
                    secret,
                    sign_name,
                    template_code,
                    region: *region,
                },
                phone,
                &message::sms_params(context, batch),
                now,
            )
        }
        (
            NotifyChannelConfig::TencentSms {
                secret_id,
                sdk_app_id,
                sign_name,
                template_id,
                region,
                ..
            },
            Recipient::Phone(phone),
        ) => {
            let secret = channel_secret.ok_or_else(not_configured)?;
            sms::tencent(
                &sms::Tencent {
                    secret_id,
                    secret_key: secret,
                    sdk_app_id,
                    sign_name,
                    template_id,
                    region: *region,
                },
                phone,
                &message::sms_params(context, batch),
                now,
            )
        }
        _ => Err(not_configured()),
    }
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(ALPHABET[(n >> shift & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), message)
        .as_ref()
        .to_vec()
}

/// RFC 3986 unreserved characters stay; everything else is `%XX`.
pub(crate) fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Random lowercase hex for nonces, MIME boundaries and Message-IDs.
pub(crate) fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    // A failed fill leaves zeros; these values need uniqueness, not secrecy.
    let _ = ring::rand::SystemRandom::new().fill(&mut buffer);
    hex(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_match_their_rfc_vectors() {
        // RFC 4648 §10.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        // RFC 4231 test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(percent_encode("a b*~/中"), "a%20b%2A~%2F%E4%B8%AD");
    }

    #[test]
    fn provider_codes_are_short_tokens_only() {
        assert_eq!(
            provider_code("isv.BUSINESS_LIMIT_CONTROL").as_deref(),
            Some("isv.BUSINESS_LIMIT_CONTROL")
        );
        assert_eq!(provider_code("19021").as_deref(), Some("19021"));
        assert!(provider_code("bad request: token=abc").is_none());
        assert!(provider_code(&"x".repeat(65)).is_none());
    }

    #[test]
    fn mismatched_channel_and_recipient_is_not_configured() {
        let error = send(
            &NotifyChannelConfig::Feishu {},
            None,
            &Recipient::Email("a@example.com"),
            &AlertBatch::test(),
            &Context {
                language: doctor_contracts::NotifyLanguage::Zh,
                link: None,
                calibrated: false,
            },
            0,
        )
        .unwrap_err();
        assert_eq!(error.class, NotifyErrorClass::NotConfigured);
    }
}
