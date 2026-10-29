//! Input rules for channel and target settings. Bot webhooks are limited to
//! the official hosts so a saved address cannot point the server anywhere
//! else; every text that reaches a protocol header is free of CR/LF.
use doctor_contracts::{
    NOTIFY_TARGET_NAME_MAX_CHARS, NotifyChannelConfig, NotifyChannelKind, NotifySettings,
};

const FEISHU_HOOKS: [&str; 2] = [
    "https://open.feishu.cn/open-apis/bot/v2/hook/",
    "https://open.larksuite.com/open-apis/bot/v2/hook/",
];
const DINGTALK_HOOK: &str = "https://oapi.dingtalk.com/robot/send?access_token=";

fn plain_text(value: &str, max_chars: usize) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() == value.len()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}

fn token(value: &str, max: usize, extra: &[u8]) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.contains(&byte))
}

fn hostname(value: &str) -> bool {
    token(value, 253, b".-") && !value.starts_with(['.', '-']) && !value.ends_with(['.', '-'])
}

pub fn target_name(name: &str) -> bool {
    plain_text(name, NOTIFY_TARGET_NAME_MAX_CHARS)
}

/// A single mailbox: `local@domain`, no spaces, angle brackets or commas.
pub fn email_address(value: &str) -> bool {
    let Some((local, domain)) = value.rsplit_once('@') else {
        return false;
    };
    value.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && local
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"<>()[],;:\"@\\".contains(&byte))
        && domain.contains('.')
        && hostname(domain)
}

/// Digits with an optional leading `+`; 6–15 digits (E.164 allows 15).
pub fn phone(value: &str) -> Option<String> {
    let compact: String = value.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    let digits = compact.strip_prefix('+').unwrap_or(&compact);
    ((6..=15).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(compact)
}

pub fn webhook(kind: NotifyChannelKind, value: &str) -> bool {
    let rest = match kind {
        NotifyChannelKind::Feishu => FEISHU_HOOKS
            .iter()
            .find_map(|prefix| value.strip_prefix(prefix)),
        NotifyChannelKind::Dingtalk => value.strip_prefix(DINGTALK_HOOK),
        _ => None,
    };
    rest.is_some_and(|token_part| token(token_part, 128, b"-_"))
}

/// Feishu / DingTalk signing secret, or a channel secret.
pub fn secret(value: &str) -> bool {
    (1..=256).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

pub fn settings(settings: &NotifySettings) -> bool {
    settings.public_url.as_deref().is_none_or(|url| {
        url.len() <= 200
            && (url.starts_with("http://") || url.starts_with("https://"))
            && url.bytes().all(|byte| byte.is_ascii_graphic())
            && !url.contains(['"', '<', '>', '\'', '`'])
    })
}

pub fn channel(config: &NotifyChannelConfig) -> bool {
    match config {
        NotifyChannelConfig::Email {
            host,
            port,
            username,
            from_address,
            from_name,
            ..
        } => {
            hostname(host)
                && *port > 0
                && username
                    .as_deref()
                    .is_none_or(|name| name.is_empty() || plain_text(name, 254))
                && email_address(from_address)
                && from_name
                    .as_deref()
                    .is_none_or(|name| name.is_empty() || plain_text(name, 60))
        }
        NotifyChannelConfig::Feishu {} | NotifyChannelConfig::Dingtalk {} => true,
        NotifyChannelConfig::AliyunSms {
            access_key_id,
            sign_name,
            template_code,
            hourly_limit,
            ..
        } => {
            token(access_key_id, 64, b"")
                && plain_text(sign_name, 30)
                && token(template_code, 40, b"_")
                && (1..=100).contains(hourly_limit)
        }
        NotifyChannelConfig::TencentSms {
            secret_id,
            sdk_app_id,
            sign_name,
            template_id,
            hourly_limit,
            ..
        } => {
            token(secret_id, 64, b"")
                && token(sdk_app_id, 20, b"")
                && sdk_app_id.bytes().all(|byte| byte.is_ascii_digit())
                && plain_text(sign_name, 30)
                && token(template_id, 20, b"")
                && template_id.bytes().all(|byte| byte.is_ascii_digit())
                && (1..=100).contains(hourly_limit)
        }
    }
}

/// `138****1024`; short numbers keep only their last two digits.
pub fn mask_phone(phone: &str) -> String {
    let chars: Vec<char> = phone.chars().collect();
    if chars.len() >= 11 {
        let tail = chars.len() - 4;
        let head = tail - 4;
        format!(
            "{}****{}",
            chars[..head].iter().collect::<String>(),
            chars[tail..].iter().collect::<String>()
        )
    } else {
        let keep = chars.len().saturating_sub(2);
        format!(
            "{}{}",
            "*".repeat(keep),
            chars[keep..].iter().collect::<String>()
        )
    }
}

/// `open.feishu.cn/…/****9f2c`: the host and the token's last four.
pub fn webhook_hint(value: &str) -> String {
    let host = value
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("");
    let tail: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{host}/…/****{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::{AliyunSmsRegion, EmailSecurity};

    #[test]
    fn n05_webhooks_only_on_official_hosts() {
        assert!(webhook(
            NotifyChannelKind::Feishu,
            "https://open.feishu.cn/open-apis/bot/v2/hook/0f1e2d3c-aaaa-bbbb"
        ));
        assert!(webhook(
            NotifyChannelKind::Feishu,
            "https://open.larksuite.com/open-apis/bot/v2/hook/abc"
        ));
        assert!(webhook(
            NotifyChannelKind::Dingtalk,
            "https://oapi.dingtalk.com/robot/send?access_token=abc123"
        ));
        for bad in [
            "http://open.feishu.cn/open-apis/bot/v2/hook/abc",
            "https://open.feishu.cn.evil.example/open-apis/bot/v2/hook/abc",
            "https://open.feishu.cn/open-apis/bot/v2/hook/abc/../x",
            "https://open.feishu.cn/open-apis/bot/v2/hook/abc\r\nX: y",
            "https://open.feishu.cn/open-apis/bot/v2/hook/",
        ] {
            assert!(!webhook(NotifyChannelKind::Feishu, bad), "{bad}");
        }
        assert!(!webhook(
            NotifyChannelKind::Dingtalk,
            "https://oapi.dingtalk.com/robot/send?access_token=abc&x=1"
        ));
        assert!(!webhook(
            NotifyChannelKind::Email,
            "https://oapi.dingtalk.com/robot/send?access_token=abc"
        ));
    }

    #[test]
    fn addresses_reject_header_injection() {
        assert!(email_address("dba-oncall@example.com"));
        for bad in [
            "a@b",
            "a b@example.com",
            "a@example.com\r\nBcc: x@y.z",
            "<a@example.com>",
            "a,b@example.com",
            "@example.com",
        ] {
            assert!(!email_address(bad), "{bad}");
        }
        assert_eq!(phone("138 0013 8000").as_deref(), Some("13800138000"));
        assert_eq!(phone("+85212345678").as_deref(), Some("+85212345678"));
        assert!(phone("12345").is_none());
        assert!(phone("+86abc").is_none());
        assert!(target_name("DBA 值班群"));
        assert!(!target_name(" padded"));
        assert!(!target_name("a\nb"));
        assert!(!target_name(&"字".repeat(41)));
    }

    #[test]
    fn channels_validate_their_own_fields() {
        let email = NotifyChannelConfig::Email {
            host: "smtp.example.com".into(),
            port: 465,
            security: EmailSecurity::Tls,
            username: Some("alerts@example.com".into()),
            from_address: "alerts@example.com".into(),
            from_name: Some("SR Doctor".into()),
        };
        assert!(channel(&email));
        let NotifyChannelConfig::Email { host, .. } = &email else {
            unreachable!()
        };
        assert_eq!(host, "smtp.example.com");
        let mut bad = email.clone();
        if let NotifyChannelConfig::Email { from_name, .. } = &mut bad {
            *from_name = Some("x\r\nBcc: y".into());
        }
        assert!(!channel(&bad));
        let aliyun = NotifyChannelConfig::AliyunSms {
            access_key_id: "LTAI5tAbc".into(),
            sign_name: "数据平台".into(),
            template_code: "SMS_480215733".into(),
            region: AliyunSmsRegion::CnHangzhou,
            hourly_limit: 10,
        };
        assert!(channel(&aliyun));
        if let NotifyChannelConfig::AliyunSms { hourly_limit, .. } = aliyun.clone() {
            assert_eq!(hourly_limit, 10);
        }
        assert!(settings(&NotifySettings {
            public_url: Some("http://10.0.0.5:8080".into()),
            ..NotifySettings::default()
        }));
        assert!(!settings(&NotifySettings {
            public_url: Some("javascript:alert(1)".into()),
            ..NotifySettings::default()
        }));
    }

    #[test]
    fn hints_hide_the_secret_part() {
        assert_eq!(mask_phone("13800001024"), "138****1024");
        assert_eq!(mask_phone("+8613800001024"), "+86138****1024");
        assert_eq!(mask_phone("123456"), "****56");
        assert_eq!(
            webhook_hint("https://open.feishu.cn/open-apis/bot/v2/hook/0f1e2d3c-9f2c"),
            "open.feishu.cn/…/****9f2c"
        );
    }
}
