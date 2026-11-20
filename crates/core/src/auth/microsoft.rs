//! Microsoft 桌面登录及 Java 版游玩权验证。
//!
//! 使用系统浏览器和回环表单回调。凭据仅供核心库和 Rust 应用壳使用。

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{self, Instant as TokioInstant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::redact::Secret;

const AUTHORIZE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const SCOPE: &str = "XboxLive.SignIn offline_access";
const MICROSOFT: Endpoints<'static> = Endpoints {
    token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
    xbox_user: "https://user.auth.xboxlive.com/user/authenticate",
    xbox_authorize: "https://xsts.auth.xboxlive.com/xsts/authorize",
    minecraft_login: "https://api.minecraftservices.com/authentication/login_with_xbox",
    entitlements: "https://api.minecraftservices.com/entitlements/mcstore",
    profile: "https://api.minecraftservices.com/minecraft/profile",
};

const CALLBACK_WINDOW: Duration = Duration::from_secs(5 * 60);
const CALLBACK_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CALLBACK_HEADERS: usize = 8 * 1024;
const MAX_CALLBACK_BODY: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE: usize = 256 * 1024;
const VERIFICATION_WINDOW: Duration = Duration::from_secs(120);
const TOKEN_MARGIN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("当前版本尚未配置 Lumi 的 Microsoft 登录")]
    Unconfigured,
    #[error("已取消 Microsoft 登录或账号验证")]
    Cancelled,
    #[error("连接或登录等待超时，请重试")]
    Timeout,
    #[error("未完成 Microsoft 授权，请重试")]
    Denied,
    #[error("Microsoft 登录已失效，请重新登录")]
    ReloginRequired,
    #[error("此 Microsoft 账号尚未创建 Xbox 档案")]
    XboxProfileRequired,
    #[error("此 Xbox 账号受到限制，请检查 Xbox 账号及家庭设置")]
    AccountRestricted,
    #[error("官方认证服务拒绝了此次访问，请稍后重试")]
    AppAccessDenied,
    #[error("此账号没有 Minecraft Java 版游玩权限")]
    NoJavaLicense,
    #[error("此账号尚未创建 Minecraft Java 版档案")]
    NoJavaProfile,
    #[error("官方认证服务返回的数据无法验证，请重试")]
    InvalidResponse,
    #[error("暂时无法验证账号，请联网重试")]
    Unavailable,
    #[error("打不开本机登录回调，请重试")]
    CallbackUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftProfile {
    pub uuid: Uuid,
    pub name: String,
    pub skin_url: Option<String>,
    pub slim: bool,
}

/// 同一官方游戏会话的 Java 版游玩权和档案。仅由完整的在线验证生成。
pub struct VerifiedSession {
    profile: MinecraftProfile,
    game_token: Secret<String>,
    refresh_token: Secret<String>,
    valid_until: Instant,
}

impl VerifiedSession {
    pub fn profile(&self) -> &MinecraftProfile {
        &self.profile
    }

    pub fn game_token(&self) -> &Secret<String> {
        &self.game_token
    }

    pub fn refresh_token(&self) -> &Secret<String> {
        &self.refresh_token
    }

    pub fn valid_until(&self) -> Instant {
        self.valid_until
    }
}

fn valid_client_id(input: Option<&str>) -> Option<&str> {
    let input = input?;
    let uuid = Uuid::parse_str(input).ok()?;
    (!uuid.is_nil() && uuid.hyphenated().to_string().eq_ignore_ascii_case(input)).then_some(input)
}

fn client_id() -> Result<&'static str, AuthError> {
    valid_client_id(option_env!("LUMI_MICROSOFT_CLIENT_ID")).ok_or(AuthError::Unconfigured)
}

pub fn configured() -> bool {
    client_id().is_ok()
}

/// 此对象持有一次性回调监听器。释放对象时关闭监听器。
pub struct PendingLogin {
    url: Url,
    listener: TcpListener,
    client_id: &'static str,
    redirect_uri: String,
    host: String,
    state: Secret<String>,
    verifier: Secret<String>,
    deadline: TokioInstant,
}

impl PendingLogin {
    /// 只用于打开系统浏览器。网址含有一次性随机状态，不交给界面和日志。
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub async fn finish(self, cancel: &CancellationToken) -> Result<VerifiedSession, AuthError> {
        let code = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(AuthError::Cancelled),
            outcome = time::timeout_at(self.deadline, wait_for_code(&self)) => {
                outcome.map_err(|_| AuthError::Timeout)??
            }
        };
        let Self {
            listener,
            client_id,
            redirect_uri,
            verifier,
            ..
        } = self;
        drop(listener);

        let client = auth_http()?;
        let form = [
            ("client_id", client_id),
            ("scope", SCOPE),
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", verifier.expose()),
        ];
        verify(&client, &MICROSOFT, &form, None, cancel).await
    }
}

pub async fn begin() -> Result<PendingLogin, AuthError> {
    start_login(client_id()?).await
}

async fn start_login(client_id: &'static str) -> Result<PendingLogin, AuthError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| AuthError::CallbackUnavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| AuthError::CallbackUnavailable)?
        .port();
    // Entra 注册了桌面端的 http://localhost，匹配回环 URI 时忽略端口。
    let host = format!("localhost:{port}");
    let redirect_uri = format!("http://{host}/");
    let state = random_secret()?;
    let verifier = random_secret()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.expose()));
    let mut url = Url::parse(AUTHORIZE).map_err(|_| AuthError::InvalidResponse)?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id),
        ("scope", SCOPE),
        ("response_type", "code"),
        ("response_mode", "form_post"),
        ("redirect_uri", &redirect_uri),
        ("state", state.expose()),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ]);
    Ok(PendingLogin {
        url,
        listener,
        client_id,
        redirect_uri,
        host,
        state,
        verifier,
        deadline: TokioInstant::now() + CALLBACK_WINDOW,
    })
}

fn random_secret() -> Result<Secret<String>, AuthError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| AuthError::CallbackUnavailable)?;
    Ok(Secret::new(URL_SAFE_NO_PAD.encode(bytes)))
}

enum Callback {
    Code(Secret<String>),
    Denied,
}

async fn wait_for_code(pending: &PendingLogin) -> Result<Secret<String>, AuthError> {
    loop {
        let (mut stream, source) = pending
            .listener
            .accept()
            .await
            .map_err(|_| AuthError::CallbackUnavailable)?;
        if !source.ip().is_loopback() {
            continue;
        }
        let callback = time::timeout(
            CALLBACK_CONNECTION_TIMEOUT,
            read_callback(&mut stream, &pending.host, pending.state.expose()),
        )
        .await
        .ok()
        .flatten();
        let (status, message) = match &callback {
            Some(Callback::Code(_)) => ("200 OK", "已收到 Microsoft 授权，请返回 Lumi。"),
            Some(Callback::Denied) => ("200 OK", "未完成 Microsoft 授权，请返回 Lumi。"),
            None => (
                "400 Bad Request",
                "这次回调无效，请继续在 Lumi 发起的页面登录。",
            ),
        };
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
            Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
            Content-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\n\
            X-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{message}",
            message.len()
        );
        // 浏览器可能已经关掉。这时已经收到的有效回调仍可使用。
        let _ = time::timeout(
            CALLBACK_CONNECTION_TIMEOUT,
            stream.write_all(reply.as_bytes()),
        )
        .await;
        match callback {
            Some(Callback::Code(code)) => return Ok(code),
            Some(Callback::Denied) => return Err(AuthError::Denied),
            None => {}
        }
    }
}

async fn read_callback(stream: &mut TcpStream, host: &str, state: &str) -> Option<Callback> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    let body_start = loop {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() >= MAX_CALLBACK_HEADERS {
            return None;
        }
    };
    if body_start > MAX_CALLBACK_HEADERS {
        return None;
    }
    let header = std::str::from_utf8(&request[..body_start - 4]).ok()?;
    let body_len = callback_header(header, host)?;
    let mut body = request.split_off(body_start);
    if body.len() > body_len {
        return None;
    }
    let bytes_read = body.len();
    body.resize(body_len, 0);
    stream.read_exact(&mut body[bytes_read..]).await.ok()?;
    let body = std::str::from_utf8(&body).ok()?;
    callback_form(body, state)
}

fn callback_header(header: &str, host: &str) -> Option<usize> {
    let mut lines = header.split("\r\n");
    if lines.next()? != "POST / HTTP/1.1" {
        return None;
    }
    let mut valid_host = false;
    let mut valid_content_type = false;
    let mut length = None;
    for line in lines {
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        if key.eq_ignore_ascii_case("host") {
            if valid_host || !host.eq_ignore_ascii_case(value) {
                return None;
            }
            valid_host = true;
        } else if key.eq_ignore_ascii_case("content-type") {
            let media_type = value.split(';').next()?.trim();
            if valid_content_type
                || !media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded")
            {
                return None;
            }
            valid_content_type = true;
        } else if key.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return None;
            }
            length = Some(value.parse::<usize>().ok()?);
        } else if key.eq_ignore_ascii_case("transfer-encoding") {
            return None;
        }
    }
    length.filter(|len| valid_host && valid_content_type && (1..=MAX_CALLBACK_BODY).contains(len))
}

fn callback_form(body: &str, expected_state: &str) -> Option<Callback> {
    if !body.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    let mut form = Url::parse("http://localhost/").ok()?;
    form.set_query(Some(body));
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in form.query_pairs() {
        let target = match key.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if target.replace(value).is_some() {
            return None;
        }
    }
    if state.as_deref() != Some(expected_state) {
        return None;
    }
    match (code, error) {
        (Some(code), None) if valid_credential(&code) => {
            Some(Callback::Code(Secret::new(code.into_owned())))
        }
        (None, Some(error)) if !error.is_empty() => Some(Callback::Denied),
        _ => None,
    }
}

pub async fn refresh(
    refresh_token: &Secret<String>,
    cancel: &CancellationToken,
) -> Result<VerifiedSession, AuthError> {
    let client_id = client_id()?;
    if !valid_credential(refresh_token.expose()) {
        return Err(AuthError::ReloginRequired);
    }
    let client = auth_http()?;
    let form = [
        ("client_id", client_id),
        ("scope", SCOPE),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    verify(&client, &MICROSOFT, &form, Some(refresh_token), cancel).await
}

struct Endpoints<'a> {
    token: &'a str,
    xbox_user: &'a str,
    xbox_authorize: &'a str,
    minecraft_login: &'a str,
    entitlements: &'a str,
    profile: &'a str,
}

fn auth_http() -> Result<Client, AuthError> {
    auth_http_builder()
        .https_only(true)
        .build()
        .map_err(network_error)
}

fn auth_http_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(REQUEST_TIMEOUT)
}

async fn verify(
    client: &Client,
    endpoints: &Endpoints<'_>,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret<String>>,
    cancel: &CancellationToken,
) -> Result<VerifiedSession, AuthError> {
    let started = Instant::now();
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        result = time::timeout(
            VERIFICATION_WINDOW,
            verify_chain(client, endpoints, form, previous_refresh, started),
        ) => result.map_err(|_| AuthError::Timeout)?,
    }
}

async fn verify_chain(
    client: &Client,
    endpoints: &Endpoints<'_>,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret<String>>,
    started: Instant,
) -> Result<VerifiedSession, AuthError> {
    let oauth = oauth_token(client, endpoints.token, form).await?;
    let refresh_token = match oauth.refresh_token {
        Some(rotated) => credential(rotated)?,
        None => previous_refresh
            .cloned()
            .ok_or(AuthError::InvalidResponse)?,
    };
    let user_token = credential(oauth.access_token)?;
    let xbox: XboxUserToken = xbox_token(
        client,
        endpoints.xbox_user,
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", user_token.expose())
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }),
    )
    .await?;
    let xbox_token_value = credential(xbox.token)?;
    let xsts: XboxToken = xbox_token(
        client,
        endpoints.xbox_authorize,
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbox_token_value.expose()] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }),
    )
    .await?;
    let (xsts_token, xsts_hash) = xsts.into_identity()?;
    let minecraft_started = Instant::now();
    let game: GameToken = parse_json(
        &official_response(
            post_json(
                client,
                endpoints.minecraft_login,
                &json!({ "identityToken": format!("XBL3.0 x={};{}", xsts_hash.expose(), xsts_token.expose()) }),
            )?,
            false,
        )
        .await?,
    )?;
    if !game.token_type.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::InvalidResponse);
    }
    let game_token = credential(game.access_token)?;
    let lifetime = Duration::from_secs(game.expires_in)
        .checked_sub(TOKEN_MARGIN)
        .ok_or(AuthError::InvalidResponse)?;
    let valid_until =
        (started + VERIFICATION_WINDOW).min(minecraft_started + lifetime.min(VERIFICATION_WINDOW));
    check_java_license(
        &official_response(
            client
                .get(endpoints.entitlements)
                .bearer_auth(game_token.expose()),
            false,
        )
        .await?,
    )?;
    let profile = read_java_profile(
        &official_response(
            client
                .get(endpoints.profile)
                .bearer_auth(game_token.expose()),
            true,
        )
        .await?,
    )?;
    if valid_until <= Instant::now() {
        return Err(AuthError::ReloginRequired);
    }
    Ok(VerifiedSession {
        profile,
        game_token,
        refresh_token,
        valid_until,
    })
}

#[derive(Deserialize)]
struct OAuthToken {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct OAuthFailure {
    error: String,
}

async fn oauth_token(
    client: &Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<OAuthToken, AuthError> {
    // 以 URL 表单规则编码请求体，认证地址本身保持无查询参数。
    let mut form_url = Url::parse(endpoint).map_err(|_| AuthError::InvalidResponse)?;
    form_url.query_pairs_mut().extend_pairs(form);
    let body = form_url
        .query()
        .ok_or(AuthError::InvalidResponse)?
        .to_owned();
    let response = send(
        client
            .post(endpoint)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body),
    )
    .await?;
    match response.status() {
        StatusCode::OK => {
            let oauth: OAuthToken = parse_json(&response_body(response).await?)?;
            if !oauth.token_type.eq_ignore_ascii_case("bearer") || oauth.expires_in == 0 {
                return Err(AuthError::InvalidResponse);
            }
            Ok(oauth)
        }
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED => {
            let failure: OAuthFailure = parse_json(&response_body(response).await?)?;
            Err(match failure.error.as_str() {
                "invalid_grant"
                | "interaction_required"
                | "login_required"
                | "consent_required" => AuthError::ReloginRequired,
                "invalid_client" | "unauthorized_client" | "invalid_scope" => {
                    AuthError::AppAccessDenied
                }
                "access_denied" => AuthError::Denied,
                "temporarily_unavailable" | "server_error" => AuthError::Unavailable,
                _ => AuthError::InvalidResponse,
            })
        }
        status => Err(status_error(status)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxUserToken {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxToken {
    token: String,
    display_claims: XboxClaims,
}

#[derive(Deserialize)]
struct XboxClaims {
    xui: Vec<XboxUser>,
}

#[derive(Deserialize)]
struct XboxUser {
    uhs: String,
}

#[derive(Deserialize)]
struct XboxFailure {
    #[serde(rename = "XErr")]
    code: u64,
}

impl XboxToken {
    fn into_identity(self) -> Result<(Secret<String>, Secret<String>), AuthError> {
        let [user] = self.display_claims.xui.as_slice() else {
            return Err(AuthError::InvalidResponse);
        };
        if user.uhs.is_empty()
            || user.uhs.len() > 128
            || !user.uhs.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(AuthError::InvalidResponse);
        }
        Ok((credential(self.token)?, Secret::new(user.uhs.clone())))
    }
}

async fn xbox_token<T: DeserializeOwned>(
    client: &Client,
    endpoint: &str,
    body: &Value,
) -> Result<T, AuthError> {
    let response =
        send(post_json(client, endpoint, body)?.header("x-xbl-contract-version", "1")).await?;
    match response.status() {
        StatusCode::OK => parse_json(&response_body(response).await?),
        status @ (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
            let bytes = response_body(response).await?;
            let code = serde_json::from_slice::<XboxFailure>(&bytes)
                .ok()
                .map(|failure| failure.code);
            Err(match code {
                Some(2148916233) => AuthError::XboxProfileRequired,
                Some(2148916227 | 2148916229 | 2148916234..=2148916238) => {
                    AuthError::AccountRestricted
                }
                _ => status_error(status),
            })
        }
        status => Err(status_error(status)),
    }
}

#[derive(Deserialize)]
struct GameToken {
    access_token: String,
    expires_in: u64,
    token_type: String,
}

#[derive(Deserialize)]
struct Entitlements {
    items: Vec<Entitlement>,
}

#[derive(Deserialize)]
struct Entitlement {
    name: String,
}

fn check_java_license(body: &[u8]) -> Result<(), AuthError> {
    let entitlements: Entitlements = parse_json(body)?;
    entitlements
        .items
        .iter()
        .any(|item| matches!(item.name.as_str(), "game_minecraft" | "product_minecraft"))
        .then_some(())
        .ok_or(AuthError::NoJavaLicense)
}

#[derive(Deserialize)]
struct Profile {
    id: String,
    name: String,
    #[serde(default)]
    skins: Vec<ProfileSkin>,
}

#[derive(Deserialize)]
struct ProfileSkin {
    state: String,
    variant: String,
    url: String,
}

fn read_java_profile(body: &[u8]) -> Result<MinecraftProfile, AuthError> {
    let profile: Profile = parse_json(body)?;
    let uuid = Uuid::parse_str(&profile.id).map_err(|_| AuthError::InvalidResponse)?;
    if uuid.is_nil()
        || !(1..=16).contains(&profile.name.len())
        || !profile
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AuthError::InvalidResponse);
    }
    let skin = profile
        .skins
        .into_iter()
        .find(|skin| skin.state == "ACTIVE");
    let (skin_url, slim) = skin.map_or((None, false), |skin| {
        let url = official_skin_url(&skin.url);
        let slim = url.is_some() && skin.variant == "SLIM";
        (url, slim)
    });
    Ok(MinecraftProfile {
        uuid,
        name: profile.name,
        skin_url,
        slim,
    })
}

fn official_skin_url(input: &str) -> Option<String> {
    let url = Url::parse(input).ok()?;
    let texture_id = url.path().strip_prefix("/texture/")?;
    (url.scheme() == "https"
        && url.host_str() == Some("textures.minecraft.net")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (1..=128).contains(&texture_id.len())
        && texture_id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| url.to_string())
}

fn credential(value: String) -> Result<Secret<String>, AuthError> {
    if !valid_credential(&value) {
        return Err(AuthError::InvalidResponse);
    }
    Ok(Secret::new(value))
}

fn valid_credential(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn network_error(error: reqwest::Error) -> AuthError {
    if error.is_timeout() {
        AuthError::Timeout
    } else {
        AuthError::Unavailable
    }
}

fn status_error(status: StatusCode) -> AuthError {
    match status {
        StatusCode::UNAUTHORIZED => AuthError::ReloginRequired,
        StatusCode::FORBIDDEN => AuthError::AppAccessDenied,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => AuthError::Timeout,
        StatusCode::TOO_MANY_REQUESTS => AuthError::Unavailable,
        status if status.is_server_error() => AuthError::Unavailable,
        _ => AuthError::InvalidResponse,
    }
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, AuthError> {
    serde_json::from_slice(body).map_err(|_| AuthError::InvalidResponse)
}

fn post_json(client: &Client, endpoint: &str, json: &Value) -> Result<RequestBuilder, AuthError> {
    let body = serde_json::to_vec(json).map_err(|_| AuthError::InvalidResponse)?;
    Ok(client
        .post(endpoint)
        .header(CONTENT_TYPE, "application/json")
        .body(body))
}

async fn send(request: RequestBuilder) -> Result<Response, AuthError> {
    request
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(network_error)
}

async fn official_response(request: RequestBuilder, profile: bool) -> Result<Vec<u8>, AuthError> {
    let response = send(request).await?;
    match response.status() {
        StatusCode::OK => response_body(response).await,
        StatusCode::NOT_FOUND if profile => Err(AuthError::NoJavaProfile),
        status => Err(status_error(status)),
    }
}

async fn response_body(mut response: Response) -> Result<Vec<u8>, AuthError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err(AuthError::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if chunk.len() > MAX_RESPONSE - body.len() {
            return Err(AuthError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
