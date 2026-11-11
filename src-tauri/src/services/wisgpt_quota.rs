//! 从 WisCode 每日额度响应生成菜单栏可展示的 Token 余额，不向前端暴露认证信息。

use crate::constants::{
    WISCODE_AUTH_FILE_NAME, WISCODE_DIRECTORY_NAME, WISGPT_DAILY_QUOTA_PATH,
    WISGPT_QUOTA_TIMEOUT_SECS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use url::Url;

/// 经过校验的每日 Token 额度；空结果表示隐藏，不表示额度为零。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WisGptQuota {
    pub remaining: f64,
    pub total: f64,
}

/// 使用给定 Home 中的 WisCode 身份查询额度；HTTP 客户端由调用方提供，沿用项目网络配置。
pub async fn fetch_daily_quota(home: &Path, client: &reqwest::Client) -> Option<WisGptQuota> {
    let auth = read_auth(home)?;
    let endpoint = auth_endpoint(&auth.host)?;
    let response = match client
        .get(endpoint)
        .bearer_auth(&auth.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        // 项目共用客户端禁用了自动解压；额度接口明确请求未压缩的 JSON。
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .timeout(Duration::from_secs(WISGPT_QUOTA_TIMEOUT_SECS))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => {
            // 不输出网络错误正文：其中可能含请求地址或认证相关信息。
            log::debug!("[WisGPTQuota] 每日额度请求失败");
            return None;
        }
    };
    let body: Value = response.json().await.ok()?;
    // 请求期间登出、换账号或删除目录时，不能将旧身份的余额重新显示出来。
    if read_auth(home).as_ref() != Some(&auth) {
        return None;
    }
    parse_daily_quota(&body)
}

/// 认证信息仅在后端短暂使用，不派生 Debug，也不将内容放入返回值和日志。
#[derive(Deserialize, PartialEq, Eq)]
struct WiscodeAuth {
    host: String,
    access_token: String,
}

/// 每次调用都重新检查目录和认证文件，保持创建、删除及重新登录后的即时恢复。
fn read_auth(home: &Path) -> Option<WiscodeAuth> {
    let directory = home.join(WISCODE_DIRECTORY_NAME);
    if !directory.is_dir() {
        return None;
    }
    let mut auth: WiscodeAuth =
        serde_json::from_slice(&std::fs::read(directory.join(WISCODE_AUTH_FILE_NAME)).ok()?)
            .ok()?;
    auth.host = auth.host.trim().to_owned();
    auth.access_token = auth.access_token.trim().to_owned();
    (!auth.host.is_empty() && !auth.access_token.is_empty()).then_some(auth)
}

/// host 通常只有域名和端口；补 HTTPS 后固定接口路径，拒绝混入路径、凭据或查询参数。
fn auth_endpoint(host: &str) -> Option<Url> {
    let mut url = Url::parse(&if host.contains("://") {
        host.to_owned()
    } else {
        format!("https://{host}")
    })
    .ok()?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    url.set_path(WISGPT_DAILY_QUOTA_PATH);
    Some(url)
}

/// 只在响应能够明确给出每日 Token 余额时返回展示数据。
pub fn parse_daily_quota(response: &Value) -> Option<WisGptQuota> {
    if response.get("error").is_some_and(|error| !error.is_null()) {
        return None;
    }
    // 允许旧接口省略成功码，但显式失败不能因仍带着 data 而展示为真实余额。
    if let Some(code) = response
        .pointer("/context/code")
        .or_else(|| response.get("code"))
    {
        if !matches!(code.as_i64(), Some(0 | 200))
            && !matches!(code.as_str(), Some("0" | "200" | "ok" | "success"))
        {
            return None;
        }
    }
    let data = response.get("data")?;
    match data.get("wis_gpt_quota_mode") {
        Some(mode) if mode.as_str() == Some("TOKEN") => {}
        Some(_) => return None,
        None => {
            // 旧接口已有 WisGPT 积分组时也不能把残留 Token 字段当成每日额度。
            if let Some(groups) = data.get("credit_groups") {
                let groups = groups.as_array()?;
                if groups.iter().any(|group| group["groupName"] == "WisGPT") {
                    return None;
                }
            }
        }
    }
    let total = quota_number(data.get("wis_gpt_total")?)?;
    let used = quota_number(data.get("wis_gpt_used")?)?;
    if total <= 0.0 {
        return None;
    }
    Some(WisGptQuota {
        remaining: (total - used).max(0.0),
        total,
    })
}

/// 同接口既有数字／数字字符串兼容规则；缺失、负数和非有限数不是零额度。
fn quota_number(value: &Value) -> Option<f64> {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 固定业务示例：总额 100M、已用 24M，前端应得到剩余 76M。
    #[test]
    fn displays_remaining_daily_tokens_from_the_existing_api() {
        let response = json!({
            "context": {"code": 0},
            "data": {
                "wis_gpt_quota_mode": "TOKEN",
                "wis_gpt_total": 100_000_000,
                "wis_gpt_used": 24_000_000
            }
        });
        assert_eq!(
            parse_daily_quota(&response),
            Some(WisGptQuota {
                remaining: 76_000_000.0,
                total: 100_000_000.0,
            })
        );
    }

    /// 跨越文件系统、真实 HTTPS、认证头与响应解析，不以内部 mock 替代请求。
    #[tokio::test]
    async fn reuses_wiscode_auth_to_request_the_real_daily_quota_path() {
        let body = json!({"context":{"code":0},"data":{
            "wis_gpt_quota_mode":"TOKEN", "wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000
        }});
        let (host, request, server) = quota_server("200 OK", body, None).await;
        let home = auth_home(&host);
        // 只有测试客户端信任自签名证书；真实命令必须使用项目默认的证书验证。
        let client = test_client();
        assert_eq!(
            fetch_daily_quota(home.path(), &client).await,
            Some(WisGptQuota {
                remaining: 76_000_000.0,
                total: 100_000_000.0
            })
        );
        let request = request.await.unwrap();
        assert!(request.starts_with("GET /api/llm/daily-token HTTP/1.1\r\n"));
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer quota-test-token\r\n"));
        server.await.unwrap();
    }

    /// 显式计费模式优先，不能把积分或无权限账号遗留的 Token 字段拿来展示。
    #[test]
    fn hides_credit_none_and_unknown_modes_even_with_residual_tokens() {
        for mode in [
            json!("CREDIT"),
            json!("NONE"),
            json!("UNKNOWN"),
            Value::Null,
        ] {
            let body = json!({"data":{"wis_gpt_quota_mode":mode,
                "wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
            assert_eq!(parse_daily_quota(&body), None);
        }
        let token = json!({"data":{"wis_gpt_quota_mode":"TOKEN",
            "credit_groups":[{"groupName":"WisGPT"}],
            "wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
        assert_eq!(parse_daily_quota(&token).unwrap().remaining, 76_000_000.0);
    }

    /// 旧接口可以没有模式并返回数字字符串；一旦已有 WisGPT 积分组就不能回退。
    #[test]
    fn supports_legacy_token_data_without_misreading_credit_groups() {
        let mut body = json!({"data":{"wis_gpt_total":"100000000","wis_gpt_used":"24000000"}});
        assert_eq!(parse_daily_quota(&body).unwrap().remaining, 76_000_000.0);
        body["data"]["credit_groups"] = json!([{"groupName":"other"}]);
        assert!(parse_daily_quota(&body).is_some());
        for groups in [json!([{"groupName":"WisGPT"}]), json!({}), Value::Null] {
            body["data"]["credit_groups"] = groups;
            assert_eq!(parse_daily_quota(&body), None);
        }
    }

    /// 缺失、负数、零总额、非有限值和业务失败不能伪装成有效余额。
    #[test]
    fn rejects_incomplete_or_failed_quota_responses() {
        let body = json!({"context":{"code":0},"data":{"wis_gpt_quota_mode":"TOKEN",
            "wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
        for total in [
            json!(0),
            json!(-1),
            json!("NaN"),
            json!("inf"),
            json!({}),
            Value::Null,
        ] {
            let mut invalid = body.clone();
            invalid["data"]["wis_gpt_total"] = total;
            assert_eq!(parse_daily_quota(&invalid), None);
        }
        for used in [json!(-1), json!("NaN"), json!("inf"), Value::Null] {
            let mut invalid = body.clone();
            invalid["data"]["wis_gpt_used"] = used;
            assert_eq!(parse_daily_quota(&invalid), None);
        }
        let mut failed = body;
        failed["context"]["code"] = json!(401);
        assert_eq!(parse_daily_quota(&failed), None);
        assert_eq!(
            parse_daily_quota(&json!({"context":{"code":0},"data":{}})),
            None
        );
    }

    /// 业务失败即使省略 code 并残留 data，也不能展示成余额。
    #[test]
    fn hides_explicit_business_errors_even_without_a_status_code() {
        let response = json!({"error":{"message":"authentication required"},
            "data":{"wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
        assert_eq!(parse_daily_quota(&response), None);
    }

    /// 真正耗尽仍是可展示结果；超过总额时钳制为零，不隐藏、不产生负数。
    #[test]
    fn keeps_exhausted_quotas_visible_and_clamps_overuse() {
        for used in [100_000_000, 120_000_000] {
            let body = json!({"data":{"wis_gpt_quota_mode":"TOKEN",
                "wis_gpt_total":100_000_000,"wis_gpt_used":used}});
            assert_eq!(
                parse_daily_quota(&body),
                Some(WisGptQuota {
                    remaining: 0.0,
                    total: 100_000_000.0
                })
            );
        }
    }

    /// 不存在配置目录、认证文件丢失或损坏时不创建文件，也不尝试联网。
    #[tokio::test]
    async fn hides_missing_or_broken_wiscode_auth_without_creating_configuration() {
        let home = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();
        assert_eq!(fetch_daily_quota(home.path(), &client).await, None);
        let directory = home.path().join(crate::constants::WISCODE_DIRECTORY_NAME);
        assert!(!directory.exists());
        std::fs::create_dir(&directory).unwrap();
        assert_eq!(fetch_daily_quota(home.path(), &client).await, None);
        for broken in [
            "not json",
            "{}",
            r#"{"host":"example.test","access_token":""}"#,
        ] {
            std::fs::write(
                directory.join(crate::constants::WISCODE_AUTH_FILE_NAME),
                broken,
            )
            .unwrap();
            assert_eq!(fetch_daily_quota(home.path(), &client).await, None);
        }
    }

    /// 即使错误响应带着看似正常的 Token 字段，HTTP 失败仍然隐藏。
    #[tokio::test]
    async fn hides_http_errors_instead_of_displaying_residual_data() {
        let body = json!({"data":{"wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
        let (host, _request, server) = quota_server("401 Unauthorized", body, None).await;
        let home = auth_home(&host);
        assert_eq!(fetch_daily_quota(home.path(), &test_client()).await, None);
        server.await.unwrap();
    }

    /// 在服务响应前换账号或删除目录，旧身份的响应必须被丢弃。
    #[tokio::test]
    async fn discards_the_response_when_auth_changes_during_the_request() {
        for remove_directory in [false, true] {
            let (release, gate) = tokio::sync::oneshot::channel();
            let body = json!({"data":{"wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
            let (host, request, server) = quota_server("200 OK", body, Some(gate)).await;
            let home = auth_home(&host);
            let path = home.path().to_path_buf();
            let pending =
                tokio::spawn(async move { fetch_daily_quota(&path, &test_client()).await });
            tokio::time::timeout(Duration::from_secs(2), request)
                .await
                .unwrap()
                .unwrap();
            let directory = home.path().join(crate::constants::WISCODE_DIRECTORY_NAME);
            let auth_file = directory.join(crate::constants::WISCODE_AUTH_FILE_NAME);
            if remove_directory {
                std::fs::remove_file(auth_file).unwrap();
                std::fs::remove_dir(directory).unwrap();
            } else {
                std::fs::write(
                    auth_file,
                    json!({"host":host,"access_token":"new-test-account"}).to_string(),
                )
                .unwrap();
            }
            release.send(()).unwrap();
            assert_eq!(pending.await.unwrap(), None);
            server.await.unwrap();
        }
    }

    /// 明文 HTTP 目标被拒绝，不能因为测试服务器没有回复而把泄露误判为失败隐藏。
    #[tokio::test]
    async fn never_sends_credentials_over_plain_http() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let home = auth_home(&format!("http://{}", listener.local_addr().unwrap()));
        assert_eq!(fetch_daily_quota(home.path(), &test_client()).await, None);
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    /// 对外部接口的重定向直接隐藏，不把查询继续转发到另一个主机或协议。
    #[tokio::test]
    async fn does_not_follow_an_upstream_redirect() {
        let destination = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let status = format!(
            "302 Found\r\nLocation: http://{}/foreign",
            destination.local_addr().unwrap()
        );
        let (host, _request, server) = quota_server(&status, json!({}), None).await;
        let home = auth_home(&host);
        assert_eq!(fetch_daily_quota(home.path(), &test_client()).await, None);
        server.await.unwrap();
        assert!(
            matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    /// TLS 与认证请求建立后冻结时钟，验证 10 秒期限，不让单测真实等待 10 秒。
    #[tokio::test]
    async fn bounds_an_unresponsive_quota_request() {
        let (_release, gate) = tokio::sync::oneshot::channel();
        let (host, request, server) = quota_server("200 OK", json!({}), Some(gate)).await;
        let home = auth_home(&host);
        let path = home.path().to_path_buf();
        let pending = tokio::spawn(async move { fetch_daily_quota(&path, &test_client()).await });
        tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(11)).await;
        let result = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .expect("额度请求应在 10 秒内结束")
            .unwrap();
        assert_eq!(result, None);
        tokio::time::resume();
        server.abort();
    }

    /// 生产 host 常是纯主机名；固定补 HTTPS，同时不接受夹带路径或 URL 凭据。
    #[tokio::test]
    async fn supports_bare_hosts_but_rejects_embedded_paths_or_credentials() {
        let body = json!({"data":{"wis_gpt_total":100_000_000,"wis_gpt_used":24_000_000}});
        let (host, _request, server) = quota_server("200 OK", body.clone(), None).await;
        let home = auth_home(host.strip_prefix("https://").unwrap());
        assert_eq!(
            fetch_daily_quota(home.path(), &test_client())
                .await
                .unwrap()
                .remaining,
            76_000_000.0
        );
        server.await.unwrap();
        for suffix in ["/unexpected", "/?query=1", "/#fragment"] {
            let (host, mut request, server) = quota_server("200 OK", body.clone(), None).await;
            let home = auth_home(&format!("{host}{suffix}"));
            assert_eq!(fetch_daily_quota(home.path(), &test_client()).await, None);
            assert!(matches!(
                request.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            server.abort();
        }
    }

    /// 仅生成隔离 Home 夹具，不使用、复制或改写真实用户的登录文件。
    fn auth_home(host: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(crate::constants::WISCODE_DIRECTORY_NAME);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join(crate::constants::WISCODE_AUTH_FILE_NAME),
            json!({"host":host,"access_token":"quota-test-token"}).to_string(),
        )
        .unwrap();
        home
    }

    /// 测试专用自签名证书白名单；生产命令不使用这项证书宽免。
    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap()
    }

    /// 用真实 TLS 和 HTTP 字节模拟外部接口，不 mock 额度解析或文件读取逻辑。
    async fn quota_server(
        status: &str,
        payload: Value,
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<String>,
        tokio::task::JoinHandle<()>,
    ) {
        use base64::Engine;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cert = base64::engine::general_purpose::STANDARD
            .decode(crate::constants::WISGPT_TEST_CERT_DER_BASE64)
            .unwrap();
        let key = base64::engine::general_purpose::STANDARD
            .decode(crate::constants::WISGPT_TEST_KEY_DER_BASE64)
            .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("https://{}", listener.local_addr().unwrap());
        let (send_request, request) = tokio::sync::oneshot::channel();
        let body = payload.to_string();
        let status = status.to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let _ = send_request.send(String::from_utf8(bytes).unwrap());
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        (host, request, server)
    }
}
