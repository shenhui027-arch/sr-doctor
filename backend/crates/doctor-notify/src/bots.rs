//! Feishu and DingTalk custom bots: one HTTPS POST of JSON to the saved
//! webhook, signed when the bot has a signing secret. Webhooks were checked
//! against the official hosts when saved and are checked again here.
use doctor_contracts::{NotifyChannelKind, NotifyErrorClass, NotifyEvent};
use serde_json::{Value, json};

use crate::message::{self, AlertBatch, Context};
use crate::{SendError, base64, hmac_sha256, percent_encode, transport, validate};

fn split(webhook: &str) -> Option<(&str, &str)> {
    let rest = webhook.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

/// Feishu: `sign = base64(HMAC-SHA256(key = "{timestamp}\n{secret}", ""))`.
pub(crate) fn feishu_sign(timestamp: i64, secret: &str) -> String {
    base64(&hmac_sha256(
        format!("{timestamp}\n{secret}").as_bytes(),
        b"",
    ))
}

/// DingTalk: `sign = urlencode(base64(HMAC-SHA256(key = secret, "{ms}\n{secret}")))`.
pub(crate) fn dingtalk_sign(timestamp_ms: i64, secret: &str) -> String {
    percent_encode(&base64(&hmac_sha256(
        secret.as_bytes(),
        format!("{timestamp_ms}\n{secret}").as_bytes(),
    )))
}

fn header_color(batch: &AlertBatch) -> &'static str {
    match batch.event {
        NotifyEvent::Test => "blue",
        NotifyEvent::Resolved => "green",
        NotifyEvent::NewAlerts if batch.highest_severity() == "high" => "red",
        NotifyEvent::NewAlerts => "orange",
    }
}

pub(crate) fn feishu_body(
    context: &Context<'_>,
    batch: &AlertBatch,
    signature: Option<(i64, String)>,
) -> Value {
    let mut elements = vec![json!({
        "tag": "div",
        "text": { "tag": "lark_md", "content": message::markdown(&Context { link: None, ..*context }, batch) }
    })];
    if let Some(link) = context.link {
        elements.push(json!({
            "tag": "action",
            "actions": [{ "tag": "button", "type": "default", "url": link,
                          "text": { "tag": "plain_text", "content": context.link_label() } }]
        }));
    }
    let mut body = json!({
        "msg_type": "interactive",
        "card": {
            "config": { "wide_screen_mode": true },
            "header": { "template": header_color(batch),
                        "title": { "tag": "plain_text", "content": context.title(batch) } },
            "elements": elements
        }
    });
    if let Some((timestamp, sign)) = signature {
        body["timestamp"] = json!(timestamp.to_string());
        body["sign"] = json!(sign);
    }
    body
}

fn post_json(webhook: &str, path: &str, body: &Value) -> Result<Value, SendError> {
    let (host, _) =
        split(webhook).ok_or_else(|| SendError::new(NotifyErrorClass::NotConfigured))?;
    let response = transport::https_post(
        host,
        path,
        &[("Content-Type", "application/json; charset=utf-8".into())],
        body.to_string().as_bytes(),
    )?;
    serde_json::from_slice(&response.body).map_err(|_| {
        if response.status >= 400 {
            SendError::coded(NotifyErrorClass::Rejected, response.status)
        } else {
            SendError::new(NotifyErrorClass::InvalidResponse)
        }
    })
}

/// `{"code":0}` (newer) or `{"StatusCode":0}` (older) means accepted.
pub(crate) fn feishu_result(reply: &Value) -> Result<(), SendError> {
    let code = reply
        .get("code")
        .or_else(|| reply.get("StatusCode"))
        .and_then(Value::as_i64)
        .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
    match code {
        0 => Ok(()),
        // Signature mismatch, timestamp out of range, IP / keyword checks.
        19021 | 19022 | 19024 => Err(SendError::coded(NotifyErrorClass::AuthFailed, code)),
        _ => Err(SendError::coded(NotifyErrorClass::Rejected, code)),
    }
}

pub(crate) fn dingtalk_result(reply: &Value) -> Result<(), SendError> {
    let code = reply
        .get("errcode")
        .and_then(Value::as_i64)
        .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
    match code {
        0 => Ok(()),
        // 310000: signature, keyword or IP check failed; 300001: bad token.
        310000 | 300001 => Err(SendError::coded(NotifyErrorClass::AuthFailed, code)),
        _ => Err(SendError::coded(NotifyErrorClass::Rejected, code)),
    }
}

pub(crate) fn feishu(
    webhook: &str,
    secret: Option<&str>,
    context: &Context<'_>,
    batch: &AlertBatch,
    now: i64,
) -> Result<(), SendError> {
    if !validate::webhook(NotifyChannelKind::Feishu, webhook) {
        return Err(SendError::new(NotifyErrorClass::NotConfigured));
    }
    let signature = secret.map(|secret| (now, feishu_sign(now, secret)));
    let (_, path) =
        split(webhook).ok_or_else(|| SendError::new(NotifyErrorClass::NotConfigured))?;
    feishu_result(&post_json(
        webhook,
        path,
        &feishu_body(context, batch, signature),
    )?)
}

pub(crate) fn dingtalk_body(context: &Context<'_>, batch: &AlertBatch) -> Value {
    // DingTalk bots with keyword security need the keyword in the text.
    let text = format!(
        "#### {}\n\n{}",
        context.title(batch),
        message::markdown(context, batch)
    );
    json!({ "msgtype": "markdown", "markdown": { "title": context.title(batch), "text": text } })
}

pub(crate) fn dingtalk_path(webhook: &str, secret: Option<&str>, now: i64) -> Option<String> {
    let (_, path) = split(webhook)?;
    Some(match secret {
        Some(secret) => {
            let ms = now * 1_000;
            format!("{path}&timestamp={ms}&sign={}", dingtalk_sign(ms, secret))
        }
        None => path.to_owned(),
    })
}

pub(crate) fn dingtalk(
    webhook: &str,
    secret: Option<&str>,
    context: &Context<'_>,
    batch: &AlertBatch,
    now: i64,
) -> Result<(), SendError> {
    if !validate::webhook(NotifyChannelKind::Dingtalk, webhook) {
        return Err(SendError::new(NotifyErrorClass::NotConfigured));
    }
    let path = dingtalk_path(webhook, secret, now)
        .ok_or_else(|| SendError::new(NotifyErrorClass::NotConfigured))?;
    dingtalk_result(&post_json(webhook, &path, &dingtalk_body(context, batch))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_contracts::NotifyLanguage;

    fn context() -> Context<'static> {
        Context {
            language: NotifyLanguage::Zh,
            link: Some("http://10.0.0.5:8080"),
            calibrated: false,
        }
    }

    #[test]
    fn n02_feishu_signature_follows_the_documented_construction() {
        // HMAC keyed by "timestamp\nsecret" over an empty message.
        let expected = base64(&hmac_sha256(b"1599360473\ndemo", b""));
        assert_eq!(feishu_sign(1_599_360_473, "demo"), expected);
        let body = feishu_body(
            &context(),
            &message::sample_batch(),
            Some((1_599_360_473, expected.clone())),
        );
        assert_eq!(body["timestamp"], "1599360473");
        assert_eq!(body["sign"], expected);
        assert_eq!(body["msg_type"], "interactive");
        assert_eq!(body["card"]["header"]["template"], "red");
        assert_eq!(
            body["card"]["elements"][1]["actions"][0]["url"],
            "http://10.0.0.5:8080"
        );
        let unsigned = feishu_body(&context(), &AlertBatch::test(), None);
        assert!(unsigned.get("sign").is_none());
        assert_eq!(unsigned["card"]["header"]["template"], "blue");
    }

    #[test]
    fn n02_dingtalk_signature_is_url_encoded_base64_in_the_query() {
        let raw = base64(&hmac_sha256(b"SECabc", b"1599360473000\nSECabc"));
        assert_eq!(
            dingtalk_sign(1_599_360_473_000, "SECabc"),
            percent_encode(&raw)
        );
        let path = dingtalk_path(
            "https://oapi.dingtalk.com/robot/send?access_token=tok",
            Some("SECabc"),
            1_599_360_473,
        )
        .unwrap();
        assert_eq!(
            path,
            format!(
                "/robot/send?access_token=tok&timestamp=1599360473000&sign={}",
                percent_encode(&raw)
            )
        );
        assert_eq!(
            dingtalk_path(
                "https://oapi.dingtalk.com/robot/send?access_token=tok",
                None,
                0
            )
            .unwrap(),
            "/robot/send?access_token=tok"
        );
        let body = dingtalk_body(&context(), &message::sample_batch());
        assert!(
            body["markdown"]["text"]
                .as_str()
                .unwrap()
                .contains("SR Doctor")
        );
    }

    #[test]
    fn n02_provider_replies_map_to_classes() {
        assert!(feishu_result(&json!({"code":0,"msg":"success"})).is_ok());
        assert!(feishu_result(&json!({"StatusCode":0})).is_ok());
        assert_eq!(
            feishu_result(&json!({"code":19021})).unwrap_err().class,
            NotifyErrorClass::AuthFailed
        );
        assert_eq!(
            feishu_result(&json!({"code":9499}))
                .unwrap_err()
                .provider_code
                .as_deref(),
            Some("9499")
        );
        assert_eq!(
            feishu_result(&json!({})).unwrap_err().class,
            NotifyErrorClass::InvalidResponse
        );
        assert!(dingtalk_result(&json!({"errcode":0,"errmsg":"ok"})).is_ok());
        assert_eq!(
            dingtalk_result(&json!({"errcode":310000}))
                .unwrap_err()
                .class,
            NotifyErrorClass::AuthFailed
        );
        assert_eq!(
            dingtalk_result(&json!({"errcode":130101}))
                .unwrap_err()
                .class,
            NotifyErrorClass::Rejected
        );
    }

    #[test]
    fn n05_unofficial_hosts_are_refused_before_any_connection() {
        let error = feishu(
            "https://example.com/open-apis/bot/v2/hook/x",
            None,
            &context(),
            &AlertBatch::test(),
            0,
        )
        .unwrap_err();
        assert_eq!(error.class, NotifyErrorClass::NotConfigured);
        let error = dingtalk(
            "https://example.com/robot/send?access_token=x",
            None,
            &context(),
            &AlertBatch::test(),
            0,
        )
        .unwrap_err();
        assert_eq!(error.class, NotifyErrorClass::NotConfigured);
    }
}
