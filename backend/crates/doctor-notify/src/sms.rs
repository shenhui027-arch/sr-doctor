//! Cloud SMS: Alibaba Cloud `SendSms` (2017-05-25, ACS3-HMAC-SHA256) and
//! Tencent Cloud `SendSms` (2021-01-11, TC3-HMAC-SHA256). Both send one
//! approved template with three variables: cluster, count, severity.
use doctor_contracts::{AliyunSmsRegion, NotifyErrorClass, TencentSmsRegion};
use serde_json::{Value, json};

use crate::{SendError, hex, hmac_sha256, percent_encode, random_hex, sha256_hex, transport};

pub(crate) struct Aliyun<'a> {
    pub access_key_id: &'a str,
    pub secret: &'a str,
    pub sign_name: &'a str,
    pub template_code: &'a str,
    pub region: AliyunSmsRegion,
}

pub(crate) struct Tencent<'a> {
    pub secret_id: &'a str,
    pub secret_key: &'a str,
    pub sdk_app_id: &'a str,
    pub sign_name: &'a str,
    pub template_id: &'a str,
    pub region: TencentSmsRegion,
}

fn utc(now: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(now).unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn aliyun_host(region: AliyunSmsRegion) -> &'static str {
    match region {
        AliyunSmsRegion::CnHangzhou => "dysmsapi.aliyuncs.com",
        AliyunSmsRegion::ApSoutheast1 => "dysmsapi.ap-southeast-1.aliyuncs.com",
    }
}

/// Mainland numbers go bare; international ones as country code + number.
fn aliyun_phone(phone: &str) -> &str {
    phone.strip_prefix('+').unwrap_or(phone)
}

/// E.164: an 11-digit mainland mobile number gets `+86`.
fn tencent_phone(phone: &str) -> String {
    if phone.starts_with('+') {
        phone.to_owned()
    } else if phone.len() == 11 && phone.starts_with('1') {
        format!("+86{phone}")
    } else {
        format!("+{phone}")
    }
}

pub(crate) struct Signed {
    pub host: &'static str,
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
    #[cfg(test)]
    pub canonical: String,
}

/// ACS3-HMAC-SHA256: parameters in the query string, empty body.
pub(crate) fn aliyun_request(
    config: &Aliyun<'_>,
    phone: &str,
    params: &[String; 3],
    now: i64,
    nonce: &str,
) -> Signed {
    let host = aliyun_host(config.region);
    let template_param =
        json!({ "cluster": params[0], "count": params[1], "level": params[2] }).to_string();
    let mut query = [
        ("PhoneNumbers", aliyun_phone(phone).to_owned()),
        ("SignName", config.sign_name.to_owned()),
        ("TemplateCode", config.template_code.to_owned()),
        ("TemplateParam", template_param),
    ];
    query.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_query = query
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let at = utc(now);
    let date = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    let payload_hash = sha256_hex(b"");
    let headers = [
        ("host", host.to_owned()),
        ("x-acs-action", "SendSms".to_owned()),
        ("x-acs-content-sha256", payload_hash.clone()),
        ("x-acs-date", date),
        ("x-acs-signature-nonce", nonce.to_owned()),
        ("x-acs-version", "2017-05-25".to_owned()),
    ];
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(";");
    let canonical = format!(
        "POST\n/\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let string_to_sign = format!("ACS3-HMAC-SHA256\n{}", sha256_hex(canonical.as_bytes()));
    let signature = hex(&hmac_sha256(
        config.secret.as_bytes(),
        string_to_sign.as_bytes(),
    ));
    let mut out: Vec<(&'static str, String)> = headers
        .into_iter()
        .filter(|(name, _)| *name != "host")
        .collect();
    out.push((
        "Authorization",
        format!(
            "ACS3-HMAC-SHA256 Credential={},SignedHeaders={signed_headers},Signature={signature}",
            config.access_key_id
        ),
    ));
    Signed {
        host,
        path: format!("/?{canonical_query}"),
        headers: out,
        body: Vec::new(),
        #[cfg(test)]
        canonical,
    }
}

pub(crate) fn aliyun_result(status: u16, reply: Option<&Value>) -> Result<(), SendError> {
    let code = reply
        .and_then(|reply| reply.get("Code"))
        .and_then(Value::as_str);
    match code {
        Some("OK") if status == 200 => Ok(()),
        Some(code)
            if code.starts_with("InvalidAccessKeyId")
                || code.starts_with("SignatureDoesNotMatch")
                || code.starts_with("Forbidden")
                || code == "IncompleteSignature" =>
        {
            Err(SendError::coded(NotifyErrorClass::AuthFailed, code))
        }
        Some(code) => Err(SendError::coded(NotifyErrorClass::Rejected, code)),
        None if status >= 400 => Err(SendError::coded(NotifyErrorClass::Rejected, status)),
        None => Err(SendError::new(NotifyErrorClass::InvalidResponse)),
    }
}

fn tencent_region(region: TencentSmsRegion) -> &'static str {
    match region {
        TencentSmsRegion::ApGuangzhou => "ap-guangzhou",
        TencentSmsRegion::ApBeijing => "ap-beijing",
        TencentSmsRegion::ApNanjing => "ap-nanjing",
    }
}

const TENCENT_HOST: &str = "sms.tencentcloudapi.com";
const TENCENT_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// TC3-HMAC-SHA256 with the documented key derivation:
/// `HMAC(HMAC(HMAC("TC3"+key, date), "sms"), "tc3_request")`.
pub(crate) fn tencent_request(
    config: &Tencent<'_>,
    phone: &str,
    params: &[String; 3],
    now: i64,
) -> Signed {
    let body = json!({
        "PhoneNumberSet": [tencent_phone(phone)],
        "SmsSdkAppId": config.sdk_app_id,
        "SignName": config.sign_name,
        "TemplateId": config.template_id,
        "TemplateParamSet": params,
    })
    .to_string();
    let at = utc(now);
    let date = format!("{:04}-{:02}-{:02}", at.year(), at.month() as u8, at.day());
    let canonical = format!(
        "POST\n/\n\ncontent-type:{TENCENT_CONTENT_TYPE}\nhost:{TENCENT_HOST}\nx-tc-action:sendsms\n\ncontent-type;host;x-tc-action\n{}",
        sha256_hex(body.as_bytes())
    );
    let scope = format!("{date}/sms/tc3_request");
    let string_to_sign = format!(
        "TC3-HMAC-SHA256\n{now}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let secret_date = hmac_sha256(
        format!("TC3{}", config.secret_key).as_bytes(),
        date.as_bytes(),
    );
    let secret_service = hmac_sha256(&secret_date, b"sms");
    let secret_signing = hmac_sha256(&secret_service, b"tc3_request");
    let signature = hex(&hmac_sha256(&secret_signing, string_to_sign.as_bytes()));
    Signed {
        host: TENCENT_HOST,
        path: "/".to_owned(),
        headers: vec![
            ("Content-Type", TENCENT_CONTENT_TYPE.to_owned()),
            ("X-TC-Action", "SendSms".to_owned()),
            ("X-TC-Version", "2021-01-11".to_owned()),
            ("X-TC-Timestamp", now.to_string()),
            ("X-TC-Region", tencent_region(config.region).to_owned()),
            (
                "Authorization",
                format!(
                    "TC3-HMAC-SHA256 Credential={}/{scope}, SignedHeaders=content-type;host;x-tc-action, Signature={signature}",
                    config.secret_id
                ),
            ),
        ],
        body: body.into_bytes(),
        #[cfg(test)]
        canonical,
    }
}

pub(crate) fn tencent_result(reply: Option<&Value>) -> Result<(), SendError> {
    let response = reply
        .and_then(|reply| reply.get("Response"))
        .ok_or_else(|| SendError::new(NotifyErrorClass::InvalidResponse))?;
    if let Some(code) = response.pointer("/Error/Code").and_then(Value::as_str) {
        let class = if code.starts_with("AuthFailure") {
            NotifyErrorClass::AuthFailed
        } else {
            NotifyErrorClass::Rejected
        };
        return Err(SendError::coded(class, code));
    }
    match response
        .pointer("/SendStatusSet/0/Code")
        .and_then(Value::as_str)
    {
        Some("Ok") => Ok(()),
        Some(code) => Err(SendError::coded(NotifyErrorClass::Rejected, code)),
        None => Err(SendError::new(NotifyErrorClass::InvalidResponse)),
    }
}

fn post(signed: &Signed) -> Result<(u16, Option<Value>), SendError> {
    let headers: Vec<(&str, String)> = signed
        .headers
        .iter()
        .map(|(name, value)| (*name, value.clone()))
        .collect();
    let response = transport::https_post(signed.host, &signed.path, &headers, &signed.body)?;
    Ok((response.status, serde_json::from_slice(&response.body).ok()))
}

pub(crate) fn aliyun(
    config: &Aliyun<'_>,
    phone: &str,
    params: &[String; 3],
    now: i64,
) -> Result<(), SendError> {
    let (status, reply) = post(&aliyun_request(config, phone, params, now, &random_hex(16)))?;
    aliyun_result(status, reply.as_ref())
}

pub(crate) fn tencent(
    config: &Tencent<'_>,
    phone: &str,
    params: &[String; 3],
    now: i64,
) -> Result<(), SendError> {
    let (_, reply) = post(&tencent_request(config, phone, params, now))?;
    tencent_result(reply.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> [String; 3] {
        ["10.0.0.5:9030".into(), "2".into(), "高".into()]
    }

    #[test]
    fn n02_aliyun_acs3_canonical_request_and_signature() {
        let config = Aliyun {
            access_key_id: "LTAI5tExample",
            secret: "secret",
            sign_name: "数据平台",
            template_code: "SMS_480215733",
            region: AliyunSmsRegion::CnHangzhou,
        };
        let signed = aliyun_request(
            &config,
            "+8613800001024",
            &params(),
            1_793_264_400,
            "nonce1",
        );
        let lines: Vec<&str> = signed.canonical.split('\n').collect();
        assert_eq!(lines[0], "POST");
        assert_eq!(lines[1], "/");
        assert!(lines[2].starts_with("PhoneNumbers=8613800001024&SignName=%E6%95%B0%E6%8D%AE%E5%B9%B3%E5%8F%B0&TemplateCode=SMS_480215733&TemplateParam=%7B%22cluster%22%3A%2210.0.0.5%3A9030%22"));
        assert_eq!(lines[3], "host:dysmsapi.aliyuncs.com");
        assert_eq!(lines[4], "x-acs-action:SendSms");
        assert_eq!(lines[6], "x-acs-date:2026-10-29T09:00:00Z");
        assert_eq!(lines[7], "x-acs-signature-nonce:nonce1");
        assert_eq!(lines[8], "x-acs-version:2017-05-25");
        assert_eq!(lines[9], "");
        assert_eq!(
            lines[10],
            "host;x-acs-action;x-acs-content-sha256;x-acs-date;x-acs-signature-nonce;x-acs-version"
        );
        assert_eq!(lines[11], sha256_hex(b""));
        let expected = hex(&hmac_sha256(
            b"secret",
            format!(
                "ACS3-HMAC-SHA256\n{}",
                sha256_hex(signed.canonical.as_bytes())
            )
            .as_bytes(),
        ));
        let auth = &signed
            .headers
            .iter()
            .find(|(name, _)| *name == "Authorization")
            .unwrap()
            .1;
        assert_eq!(
            auth,
            &format!(
                "ACS3-HMAC-SHA256 Credential=LTAI5tExample,SignedHeaders={},Signature={expected}",
                lines[10]
            )
        );
        assert!(signed.path.starts_with("/?PhoneNumbers="));
        assert_eq!(
            aliyun_request(
                &Aliyun {
                    region: AliyunSmsRegion::ApSoutheast1,
                    ..config
                },
                "1",
                &params(),
                0,
                "n"
            )
            .host,
            "dysmsapi.ap-southeast-1.aliyuncs.com"
        );
    }

    #[test]
    fn n02_tencent_tc3_derives_the_signing_key_by_date_and_service() {
        let config = Tencent {
            secret_id: "AKIDexample",
            secret_key: "key",
            sdk_app_id: "1400000000",
            sign_name: "数据平台",
            template_id: "123456",
            region: TencentSmsRegion::ApGuangzhou,
        };
        let signed = tencent_request(&config, "13800001024", &params(), 1_793_264_400);
        let body: Value = serde_json::from_slice(&signed.body).unwrap();
        assert_eq!(body["PhoneNumberSet"][0], "+8613800001024");
        assert_eq!(
            body["TemplateParamSet"],
            json!(["10.0.0.5:9030", "2", "高"])
        );
        assert_eq!(
            signed.canonical,
            format!(
                "POST\n/\n\ncontent-type:application/json; charset=utf-8\nhost:sms.tencentcloudapi.com\nx-tc-action:sendsms\n\ncontent-type;host;x-tc-action\n{}",
                sha256_hex(&signed.body)
            )
        );
        let string_to_sign = format!(
            "TC3-HMAC-SHA256\n1793264400\n2026-10-29/sms/tc3_request\n{}",
            sha256_hex(signed.canonical.as_bytes())
        );
        let key = hmac_sha256(
            &hmac_sha256(&hmac_sha256(b"TC3key", b"2026-10-29"), b"sms"),
            b"tc3_request",
        );
        let signature = hex(&hmac_sha256(&key, string_to_sign.as_bytes()));
        let auth = &signed
            .headers
            .iter()
            .find(|(name, _)| *name == "Authorization")
            .unwrap()
            .1;
        assert_eq!(
            auth,
            &format!(
                "TC3-HMAC-SHA256 Credential=AKIDexample/2026-10-29/sms/tc3_request, SignedHeaders=content-type;host;x-tc-action, Signature={signature}"
            )
        );
        assert_eq!(tencent_phone("+85212345678"), "+85212345678");
        assert_eq!(tencent_phone("85212345678"), "+85212345678");
    }

    #[test]
    fn n02_sms_replies_map_to_classes() {
        assert!(aliyun_result(200, Some(&json!({"Code":"OK"}))).is_ok());
        assert_eq!(
            aliyun_result(200, Some(&json!({"Code":"isv.BUSINESS_LIMIT_CONTROL"})))
                .unwrap_err()
                .provider_code
                .as_deref(),
            Some("isv.BUSINESS_LIMIT_CONTROL")
        );
        assert_eq!(
            aliyun_result(400, Some(&json!({"Code":"SignatureDoesNotMatch"})))
                .unwrap_err()
                .class,
            NotifyErrorClass::AuthFailed
        );
        assert_eq!(
            aliyun_result(503, None)
                .unwrap_err()
                .provider_code
                .as_deref(),
            Some("503")
        );
        assert!(
            tencent_result(Some(&json!({"Response":{"SendStatusSet":[{"Code":"Ok"}]}}))).is_ok()
        );
        assert_eq!(
            tencent_result(Some(
                &json!({"Response":{"Error":{"Code":"AuthFailure.SignatureFailure"}}})
            ))
            .unwrap_err()
            .class,
            NotifyErrorClass::AuthFailed
        );
        assert_eq!(tencent_result(Some(&json!({"Response":{"SendStatusSet":[{"Code":"LimitExceeded.PhoneNumberDailyLimit"}]}}))).unwrap_err().class, NotifyErrorClass::Rejected);
        assert_eq!(
            tencent_result(None).unwrap_err().class,
            NotifyErrorClass::InvalidResponse
        );
    }
}
