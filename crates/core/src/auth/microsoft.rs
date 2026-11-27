//! Microsoft 桌面登录及 Java 版游玩权验证。
//!
//! 使用系统浏览器和双栈回环表单回调（OAuth 2.1 + PKCE S256 + form_post）。凭据仅供核心库和 Rust 应用壳使用。

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV6};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::{self, Instant as TokioInstant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::net::http::USER_AGENT;
use crate::redact::{Secret, constant_time_eq};

const AUTHORIZE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const SCOPE: &str = "XboxLive.SignIn offline_access";
const MICROSOFT: Endpoints<'static> = Endpoints {
    token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
    xbox_user: "https://user.auth.xboxlive.com/user/authenticate",
    xbox_authorize: "https://xsts.auth.xboxlive.com/xsts/authorize",
    minecraft_login: "https://api.minecraftservices.com/authentication/login_with_xbox",
    entitlements: "https://api.minecraftservices.com/entitlements/mcstore",
    profile: "https://api.minecraftservices.com/minecraft/profile",
    skins: "https://api.minecraftservices.com/minecraft/profile/skins",
    skins_active: "https://api.minecraftservices.com/minecraft/profile/skins/active",
    capes_active: "https://api.minecraftservices.com/minecraft/profile/capes/active",
};

const CALLBACK_WINDOW: Duration = Duration::from_secs(5 * 60);
const CALLBACK_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CALLBACK_CONCURRENCY: usize = 8;
const MAX_CALLBACK_HEADERS: usize = 8 * 1024;
const MAX_CALLBACK_BODY: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE: usize = 256 * 1024;
const VERIFICATION_WINDOW: Duration = Duration::from_secs(120);
const TOKEN_MARGIN: Duration = Duration::from_secs(10);
const RETRY_DELAYS_MS: [u64; 2] = [350, 900];

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
    #[error("此 Xbox 账号已被封禁或处于强制限制状态")]
    XboxAccountBanned,
    #[error("所在国家或地区的 Xbox Live 服务暂不可用")]
    XboxRegionBlocked,
    #[error("此账号为未成年人/家庭组受限账号，需要家长在 Microsoft 家庭组中批准")]
    XboxChildAccount,
    #[error("此 Xbox 账号受到限制，请检查 Xbox 账号及家庭设置")]
    AccountRestricted,
    #[error("官方认证服务拒绝了此次访问，请稍后重试")]
    AppAccessDenied,
    #[error("此账号没有 Minecraft Java 版游玩权限")]
    NoJavaLicense,
    #[error("此账号尚未创建 Minecraft Java 版档案")]
    NoJavaProfile,
    #[error("操作过于频繁，请稍后再试")]
    RateLimited,
    #[error("上传的皮肤图片不符合 Mojang 官方规范")]
    SkinUploadRejected,
    #[error("官方认证服务返回的数据无法验证，请重试")]
    InvalidResponse,
    #[error("暂时无法验证账号，请联网重试")]
    Unavailable,
    #[error("打不开本机登录回调，请重试")]
    CallbackUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum AuthStage {
    WaitingBrowser,
    ExchangingToken,
    AuthenticatingXbox,
    AuthorizingXsts,
    VerifyingMinecraft,
    SyncingProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOptions {
    pub lang: String,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            lang: "zh-CN".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftCape {
    pub id: String,
    pub alias: String,
    pub url: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftProfile {
    pub uuid: Uuid,
    pub name: String,
    pub skin_url: Option<String>,
    pub slim: bool,
    pub capes: Vec<MinecraftCape>,
}

/// 同一官方游戏会话的 Java 版游玩权和档案。仅由完整的在线验证生成。
#[derive(Clone)]
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

    #[cfg(test)]
    pub(crate) fn for_test(profile: MinecraftProfile) -> Self {
        Self {
            profile,
            game_token: Secret::new("test-game-token".to_owned()),
            refresh_token: Secret::new("test-refresh-token".to_owned()),
            valid_until: Instant::now() + Duration::from_secs(60),
        }
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

pub fn active_callback_mode() -> &'static str {
    if option_env!("LUMI_MICROSOFT_CALLBACK_PATH") == Some("/lumi-callback") {
        "loopback-ip-127.0.0.1/lumi-callback"
    } else {
        "dualstack-localhost"
    }
}

fn callback_target() -> (&'static str, &'static str) {
    if option_env!("LUMI_MICROSOFT_CALLBACK_PATH") == Some("/lumi-callback") {
        ("127.0.0.1", "/lumi-callback")
    } else {
        ("localhost", "/")
    }
}

/// 此对象持有一次性回调监听器。释放对象时关闭监听器。
pub struct PendingLogin {
    url: Url,
    listener: TcpListener,
    listener_v6: Option<TcpListener>,
    client_id: &'static str,
    redirect_uri: String,
    host: String,
    callback_path: &'static str,
    lang: String,
    state: Secret<String>,
    verifier: Secret<String>,
    deadline: TokioInstant,
}

impl PendingLogin {
    /// 只用于打开系统浏览器或后端直接复制。网址含有一次性随机状态，不交给界面和日志。
    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn expires_in_secs(&self) -> u32 {
        CALLBACK_WINDOW.as_secs() as u32
    }

    pub async fn finish(self, cancel: &CancellationToken) -> Result<VerifiedSession, AuthError> {
        let client = auth_http()?;
        self.finish_with_client(&client, cancel, |_| {}).await
    }

    pub async fn finish_with_client(
        self,
        client: &Client,
        cancel: &CancellationToken,
        on_stage: impl Fn(AuthStage) + Send + Sync,
    ) -> Result<VerifiedSession, AuthError> {
        let code = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(AuthError::Cancelled),
            outcome = time::timeout_at(self.deadline, wait_for_code(&self)) => {
                outcome.map_err(|_| AuthError::Timeout)??
            }
        };
        let Self {
            listener,
            listener_v6,
            client_id,
            redirect_uri,
            verifier,
            ..
        } = self;
        drop(listener_v6);
        drop(listener);

        on_stage(AuthStage::ExchangingToken);
        let form = [
            ("client_id", client_id),
            ("scope", SCOPE),
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", verifier.expose()),
        ];
        verify(client, &MICROSOFT, &form, None, cancel, &on_stage).await
    }
}

pub async fn begin() -> Result<PendingLogin, AuthError> {
    begin_with_options(&LoginOptions::default()).await
}

pub async fn begin_with_options(options: &LoginOptions) -> Result<PendingLogin, AuthError> {
    start_login(client_id()?, options).await
}

async fn start_login(
    client_id: &'static str,
    options: &LoginOptions,
) -> Result<PendingLogin, AuthError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| AuthError::CallbackUnavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| AuthError::CallbackUnavailable)?
        .port();
    let (host_name, callback_path) = callback_target();
    let listener_v6 = if host_name == "localhost" {
        TcpListener::bind(SocketAddrV6::new(Ipv6Addr::LOCALHOST, port, 0, 0))
            .await
            .ok()
    } else {
        None
    };
    let host = format!("{host_name}:{port}");
    let redirect_uri = format!("http://{host}{callback_path}");
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
        listener_v6,
        client_id,
        redirect_uri,
        host,
        callback_path,
        lang: options.lang.clone(),
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

enum ParsedHttp {
    Favicon,
    Form(Option<Callback>),
}

async fn accept_next(
    v4: &TcpListener,
    v6: Option<&TcpListener>,
) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
    match v6 {
        Some(v6) => tokio::select! {
            res = v4.accept() => res,
            res = v6.accept() => res,
        },
        None => v4.accept().await,
    }
}

async fn wait_for_code(pending: &PendingLogin) -> Result<Secret<String>, AuthError> {
    let semaphore = Arc::new(Semaphore::new(MAX_CALLBACK_CONCURRENCY));
    let (tx, mut rx) = mpsc::channel::<Callback>(4);
    let stop = CancellationToken::new();

    loop {
        tokio::select! {
            biased;
            outcome = rx.recv() => {
                stop.cancel();
                match outcome {
                    Some(Callback::Code(code)) => return Ok(code),
                    Some(Callback::Denied) => return Err(AuthError::Denied),
                    None => return Err(AuthError::CallbackUnavailable),
                }
            }
            accepted = accept_next(&pending.listener, pending.listener_v6.as_ref()) => {
                let (mut stream, source) = accepted.map_err(|_| AuthError::CallbackUnavailable)?;
                if !source.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = semaphore.clone().acquire_owned().await else {
                    continue;
                };
                let host = pending.host.clone();
                let path = pending.callback_path;
                let expected_state = pending.state.clone();
                let lang = pending.lang.clone();
                let tx = tx.clone();
                let child_stop = stop.child_token();
                tokio::spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        biased;
                        () = child_stop.cancelled() => {},
                        () = handle_callback_connection(
                            &mut stream,
                            &host,
                            path,
                            expected_state.expose(),
                            &lang,
                            &tx,
                        ) => {},
                    }
                });
            }
        }
    }
}

async fn handle_callback_connection(
    stream: &mut TcpStream,
    host: &str,
    path: &str,
    expected_state: &str,
    lang: &str,
    tx: &mpsc::Sender<Callback>,
) {
    let parsed = time::timeout(
        CALLBACK_CONNECTION_TIMEOUT,
        read_callback(stream, host, path, expected_state),
    )
    .await
    .ok()
    .unwrap_or(ParsedHttp::Form(None));

    match parsed {
        ParsedHttp::Favicon => {
            let reply =
                "HTTP/1.1 204 No Content\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
            let _ = time::timeout(
                CALLBACK_CONNECTION_TIMEOUT,
                stream.write_all(reply.as_bytes()),
            )
            .await;
            let _ = stream.shutdown().await;
        }
        ParsedHttp::Form(callback) => {
            let (status, kind) = match &callback {
                Some(Callback::Code(_)) => ("200 OK", HtmlOutcome::Success),
                Some(Callback::Denied) => ("200 OK", HtmlOutcome::Denied),
                None => ("400 Bad Request", HtmlOutcome::Invalid),
            };
            let body = branded_callback_html(lang, kind);
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
                Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
                Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'\r\n\
                X-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = time::timeout(
                CALLBACK_CONNECTION_TIMEOUT,
                stream.write_all(reply.as_bytes()),
            )
            .await;
            let _ = stream.shutdown().await;
            if let Some(cb) = callback {
                let _ = tx.send(cb).await;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum HtmlOutcome {
    Success,
    Denied,
    Invalid,
}

fn branded_callback_html(lang: &str, outcome: HtmlOutcome) -> String {
    let is_en = lang.eq_ignore_ascii_case("en");
    let (title, badge, heading, body, hint) = match (is_en, outcome) {
        (true, HtmlOutcome::Success) => (
            "Microsoft Sign-In Complete · Lumi Launcher",
            "Verified Official Authorization",
            "Microsoft authorization received",
            "Returning to Lumi Launcher to finish verifying your Xbox and Minecraft Java profile.",
            "You can safely close this browser tab.",
        ),
        (true, HtmlOutcome::Denied) => (
            "Sign-In Canceled · Lumi Launcher",
            "Official Authorization",
            "Microsoft sign-in was not completed",
            "No account changes were made. Return to Lumi Launcher if you would like to try again.",
            "You can safely close this browser tab.",
        ),
        (true, HtmlOutcome::Invalid) => (
            "Invalid Callback · Lumi Launcher",
            "Security Check",
            "This callback request is invalid",
            "Please continue signing in only from the browser window opened by Lumi Launcher.",
            "Never enter account codes into third-party websites.",
        ),
        (false, HtmlOutcome::Success) => (
            "Microsoft 登录完成 · Lumi Launcher",
            "官方正版 OAuth 2.1 授权",
            "已收到 Microsoft 安全授权",
            "正在切回 Lumi Launcher 继续核验 Xbox 与 Minecraft Java 版游玩权限。",
            "你可以安全关闭此浏览器标签页。",
        ),
        (false, HtmlOutcome::Denied) => (
            "已取消登录 · Lumi Launcher",
            "官方正版授权",
            "未完成 Microsoft 授权",
            "未对任何账号产生改动；如需重新登录，请回到 Lumi Launcher 再试。",
            "你可以安全关闭此浏览器标签页。",
        ),
        (false, HtmlOutcome::Invalid) => (
            "回调无效 · Lumi Launcher",
            "安全校验拦截",
            "本次回调请求无效",
            "请在由 Lumi Launcher 唤起的浏览器页面中完成登录。",
            "官方唯一门户为 lumilauncher.org，切勿向陌生网页透露授权信息。",
        ),
    };
    let html_lang = if is_en { "en" } else { "zh-CN" };
    let accent = match outcome {
        HtmlOutcome::Success => "oklch(0.62 0.16 245)",
        HtmlOutcome::Denied => "oklch(0.65 0.14 75)",
        HtmlOutcome::Invalid => "oklch(0.60 0.18 25)",
    };
    format!(
        "<!doctype html><html lang=\"{html_lang}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title>\
<style>:root{{color-scheme:light dark;--bg:oklch(0.97 0.008 245);--card:oklch(1 0 0);--fg:oklch(0.20 0.02 250);--muted:oklch(0.48 0.02 250);--line:oklch(0.88 0.01 250)}}\
@media(prefers-color-scheme:dark){{:root{{--bg:oklch(0.16 0.015 250);--card:oklch(0.21 0.018 250);--fg:oklch(0.94 0.01 250);--muted:oklch(0.72 0.015 250);--line:oklch(0.30 0.02 250)}}}}\
*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:var(--bg);color:var(--fg);font:15px/1.6 system-ui,-apple-system,\"Segoe UI\",sans-serif}}\
main{{width:100%;max-width:440px;padding:28px;border-radius:18px;background:var(--card);border:1px solid var(--line);box-shadow:0 18px 48px oklch(0 0 0 / 0.12)}}\
.badge{{display:inline-block;padding:4px 10px;border-radius:999px;font-size:12px;font-weight:600;color:{accent};border:1px solid {accent}}}\
h1{{margin:14px 0 8px;font-size:20px;line-height:1.3}}p{{margin:0 0 12px;color:var(--muted)}}\
.hint{{font-size:13px;padding-top:12px;border-top:1px solid var(--line)}}</style></head>\
<body><main><span class=\"badge\">{badge}</span><h1>{heading}</h1><p>{body}</p><p class=\"hint\">{hint}</p></main></body></html>"
    )
}

async fn read_callback(stream: &mut TcpStream, host: &str, path: &str, state: &str) -> ParsedHttp {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    let body_start = loop {
        let Ok(read) = stream.read(&mut buffer).await else {
            return ParsedHttp::Form(None);
        };
        if read == 0 {
            return ParsedHttp::Form(None);
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() >= MAX_CALLBACK_HEADERS {
            return ParsedHttp::Form(None);
        }
    };
    if body_start > MAX_CALLBACK_HEADERS {
        return ParsedHttp::Form(None);
    }
    let Ok(header) = std::str::from_utf8(&request[..body_start - 4]) else {
        return ParsedHttp::Form(None);
    };
    if header.starts_with("GET /favicon.ico HTTP/1.1\r\n") {
        return ParsedHttp::Favicon;
    }
    let Some(body_len) = callback_header(header, host, path) else {
        return ParsedHttp::Form(None);
    };
    let mut body = request.split_off(body_start);
    if body.len() > body_len {
        return ParsedHttp::Form(None);
    }
    let bytes_read = body.len();
    body.resize(body_len, 0);
    if stream.read_exact(&mut body[bytes_read..]).await.is_err() {
        return ParsedHttp::Form(None);
    }
    let Ok(body) = std::str::from_utf8(&body) else {
        return ParsedHttp::Form(None);
    };
    ParsedHttp::Form(callback_form(body, state))
}

fn callback_header(header: &str, host: &str, path: &str) -> Option<usize> {
    let mut lines = header.split("\r\n");
    let expected_request_line = format!("POST {path} HTTP/1.1");
    if lines.next()? != expected_request_line {
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
    if expected_state.is_empty() || !body.bytes().all(|byte| byte.is_ascii_graphic()) {
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
    let received_state = state?;
    if !constant_time_eq(received_state.as_bytes(), expected_state.as_bytes()) {
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
    let client = auth_http()?;
    refresh_with_client(&client, refresh_token, cancel, |_| {}).await
}

pub async fn refresh_with_client(
    client: &Client,
    refresh_token: &Secret<String>,
    cancel: &CancellationToken,
    on_stage: impl Fn(AuthStage) + Send + Sync,
) -> Result<VerifiedSession, AuthError> {
    let client_id = client_id()?;
    if !valid_credential(refresh_token.expose()) {
        return Err(AuthError::ReloginRequired);
    }
    on_stage(AuthStage::ExchangingToken);
    let form = [
        ("client_id", client_id),
        ("scope", SCOPE),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    verify(
        client,
        &MICROSOFT,
        &form,
        Some(refresh_token),
        cancel,
        &on_stage,
    )
    .await
}

pub async fn fetch_profile(
    client: &Client,
    game_token: &Secret<String>,
    cancel: &CancellationToken,
) -> Result<MinecraftProfile, AuthError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        res = send_with_retry("profile", true, || {
            Ok(client
                .get(MICROSOFT.profile)
                .header(ACCEPT, "application/json")
                .bearer_auth(game_token.expose()))
        }) => {
            let response = res?;
            match response.status() {
                StatusCode::OK => read_java_profile(&response_body(response).await?),
                StatusCode::NOT_FOUND => Err(AuthError::NoJavaProfile),
                status => {
                    tracing::warn!(stage = "profile", status = status.as_u16(), "Mojang 档案接口返回异常状态");
                    Err(status_error(status))
                }
            }
        }
    }
}

pub async fn upload_skin(
    client: &Client,
    game_token: &Secret<String>,
    png_bytes: &[u8],
    slim: bool,
    cancel: &CancellationToken,
) -> Result<MinecraftProfile, AuthError> {
    let (boundary, body) = build_skin_multipart(png_bytes, slim)?;
    let content_type = format!("multipart/form-data; boundary={boundary}");
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        res = send_once("upload_skin", || {
            Ok(client
                .post(MICROSOFT.skins)
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, &content_type)
                .bearer_auth(game_token.expose())
                .body(body.clone()))
        }) => {
            let response = res?;
            match response.status() {
                StatusCode::OK => {
                    let bytes = response_body(response).await?;
                    if bytes.is_empty() {
                        fetch_profile(client, game_token, cancel).await
                    } else {
                        read_java_profile(&bytes)
                    }
                }
                StatusCode::NO_CONTENT => fetch_profile(client, game_token, cancel).await,
                StatusCode::BAD_REQUEST => {
                    tracing::warn!(stage = "upload_skin", status = 400, "Mojang 拒绝了上传的皮肤文件");
                    Err(AuthError::SkinUploadRejected)
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    tracing::warn!(stage = "upload_skin", status = 429, "Mojang 皮肤接口限流");
                    Err(AuthError::RateLimited)
                }
                status => {
                    tracing::warn!(stage = "upload_skin", status = status.as_u16(), "Mojang 上传皮肤失败");
                    Err(status_error(status))
                }
            }
        }
    }
}

pub async fn reset_skin(
    client: &Client,
    game_token: &Secret<String>,
    cancel: &CancellationToken,
) -> Result<MinecraftProfile, AuthError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        res = send_once("reset_skin", || {
            Ok(client
                .delete(MICROSOFT.skins_active)
                .header(ACCEPT, "application/json")
                .bearer_auth(game_token.expose()))
        }) => {
            let response = res?;
            match response.status() {
                StatusCode::OK | StatusCode::NO_CONTENT => fetch_profile(client, game_token, cancel).await,
                StatusCode::TOO_MANY_REQUESTS => Err(AuthError::RateLimited),
                status => {
                    tracing::warn!(stage = "reset_skin", status = status.as_u16(), "Mojang 重置皮肤失败");
                    Err(status_error(status))
                }
            }
        }
    }
}

pub async fn set_active_cape(
    client: &Client,
    game_token: &Secret<String>,
    cape_id: Option<&str>,
    cancel: &CancellationToken,
) -> Result<MinecraftProfile, AuthError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        res = send_once("cape_set", || {
            if let Some(id) = cape_id {
                post_json_method(
                    client.put(MICROSOFT.capes_active),
                    &json!({ "capeId": id }),
                )
                .map(|builder| builder.bearer_auth(game_token.expose()))
            } else {
                Ok(client
                    .delete(MICROSOFT.capes_active)
                    .header(ACCEPT, "application/json")
                    .bearer_auth(game_token.expose()))
            }
        }) => {
            let response = res?;
            match response.status() {
                StatusCode::OK => {
                    let bytes = response_body(response).await?;
                    if bytes.is_empty() {
                        fetch_profile(client, game_token, cancel).await
                    } else {
                        read_java_profile(&bytes)
                    }
                }
                StatusCode::NO_CONTENT => fetch_profile(client, game_token, cancel).await,
                StatusCode::TOO_MANY_REQUESTS => Err(AuthError::RateLimited),
                status => {
                    tracing::warn!(stage = "cape_set", status = status.as_u16(), "Mojang 切换披风失败");
                    Err(status_error(status))
                }
            }
        }
    }
}

fn build_skin_multipart(png_bytes: &[u8], slim: bool) -> Result<(String, Vec<u8>), AuthError> {
    let mut rand = [0u8; 16];
    getrandom::fill(&mut rand).map_err(|_| AuthError::InvalidResponse)?;
    let boundary: String = rand.iter().map(|b| format!("{b:02x}")).collect();
    let variant = if slim { "slim" } else { "classic" };
    let mut body = Vec::with_capacity(png_bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{variant}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\n\
             Content-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(png_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((boundary, body))
}

struct Endpoints<'a> {
    token: &'a str,
    xbox_user: &'a str,
    xbox_authorize: &'a str,
    minecraft_login: &'a str,
    entitlements: &'a str,
    profile: &'a str,
    skins: &'a str,
    skins_active: &'a str,
    capes_active: &'a str,
}

fn auth_http() -> Result<Client, AuthError> {
    auth_http_builder()
        .https_only(true)
        .build()
        .map_err(network_error)
}

fn auth_http_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(REQUEST_TIMEOUT)
}

async fn verify(
    client: &Client,
    endpoints: &Endpoints<'_>,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret<String>>,
    cancel: &CancellationToken,
    on_stage: &(impl Fn(AuthStage) + Send + Sync),
) -> Result<VerifiedSession, AuthError> {
    let started = Instant::now();
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AuthError::Cancelled),
        result = time::timeout(
            VERIFICATION_WINDOW,
            verify_chain(client, endpoints, form, previous_refresh, started, on_stage),
        ) => result.map_err(|_| AuthError::Timeout)?,
    }
}

async fn verify_chain(
    client: &Client,
    endpoints: &Endpoints<'_>,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret<String>>,
    started: Instant,
    on_stage: &(impl Fn(AuthStage) + Send + Sync),
) -> Result<VerifiedSession, AuthError> {
    on_stage(AuthStage::ExchangingToken);
    let can_retry_oauth = previous_refresh.is_some();
    let oauth = oauth_token(client, endpoints.token, form, can_retry_oauth).await?;
    let refresh_token = match oauth.refresh_token {
        Some(rotated) => credential(rotated)?,
        None => previous_refresh
            .cloned()
            .ok_or(AuthError::InvalidResponse)?,
    };
    let user_token = credential(oauth.access_token)?;

    on_stage(AuthStage::AuthenticatingXbox);
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
        "xbox_user",
    )
    .await?;
    let xbox_token_value = credential(xbox.token)?;

    on_stage(AuthStage::AuthorizingXsts);
    let xsts: XboxToken = xbox_token(
        client,
        endpoints.xbox_authorize,
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbox_token_value.expose()] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }),
        "xsts",
    )
    .await?;
    let (xsts_token, xsts_hash) = xsts.into_identity()?;

    on_stage(AuthStage::VerifyingMinecraft);
    let minecraft_started = Instant::now();
    let game: GameToken = parse_json(
        &official_response(
            "minecraft_login",
            true,
            || {
                post_json(
                    client,
                    endpoints.minecraft_login,
                    &json!({ "identityToken": format!("XBL3.0 x={};{}", xsts_hash.expose(), xsts_token.expose()) }),
                )
            },
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
            "entitlements",
            true,
            || {
                Ok(client
                    .get(endpoints.entitlements)
                    .bearer_auth(game_token.expose()))
            },
            false,
        )
        .await?,
    )?;

    on_stage(AuthStage::SyncingProfile);
    let profile = read_java_profile(
        &official_response(
            "profile",
            true,
            || {
                Ok(client
                    .get(endpoints.profile)
                    .bearer_auth(game_token.expose()))
            },
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
    retry_transient: bool,
) -> Result<OAuthToken, AuthError> {
    // 以 URL 表单规则编码请求体，认证地址本身保持无查询参数。
    let mut form_url = Url::parse(endpoint).map_err(|_| AuthError::InvalidResponse)?;
    form_url.query_pairs_mut().extend_pairs(form);
    let body = form_url
        .query()
        .ok_or(AuthError::InvalidResponse)?
        .to_owned();

    let mut attempt = 0;
    let response = loop {
        let req = client
            .post(endpoint)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body.clone());
        match req.send().await {
            Ok(resp) => {
                if retry_transient
                    && matches!(
                        resp.status(),
                        StatusCode::TOO_MANY_REQUESTS
                            | StatusCode::INTERNAL_SERVER_ERROR
                            | StatusCode::BAD_GATEWAY
                            | StatusCode::SERVICE_UNAVAILABLE
                            | StatusCode::GATEWAY_TIMEOUT
                    )
                    && let Some(delay) = RETRY_DELAYS_MS.get(attempt)
                {
                    tracing::warn!(
                        stage = "oauth_token",
                        status = resp.status().as_u16(),
                        attempt,
                        "Microsoft OAuth 临时状态异常，短暂退避后重试"
                    );
                    attempt += 1;
                    time::sleep(Duration::from_millis(*delay)).await;
                    continue;
                }
                break resp;
            }
            Err(err) => {
                // 授权码是一次性的：仅在未建连成功（is_connect）时允许重发，刷新令牌可对瞬时网络异常重发。
                let safe_to_retry = err.is_connect() || (retry_transient && err.is_timeout());
                if safe_to_retry && let Some(delay) = RETRY_DELAYS_MS.get(attempt) {
                    tracing::warn!(
                        stage = "oauth_token",
                        kind = transport_kind(&err),
                        attempt,
                        "Microsoft OAuth 建连瞬时失败，短暂退避后重试"
                    );
                    attempt += 1;
                    time::sleep(Duration::from_millis(*delay)).await;
                    continue;
                }
                tracing::warn!(
                    stage = "oauth_token",
                    kind = transport_kind(&err),
                    "Microsoft OAuth 网络请求失败"
                );
                return Err(network_error(err));
            }
        }
    };

    match response.status() {
        StatusCode::OK => {
            let oauth: OAuthToken = parse_json(&response_body(response).await?)?;
            if !oauth.token_type.eq_ignore_ascii_case("bearer") || oauth.expires_in == 0 {
                return Err(AuthError::InvalidResponse);
            }
            Ok(oauth)
        }
        status @ (StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED) => {
            let failure: OAuthFailure = parse_json(&response_body(response).await?)?;
            tracing::warn!(
                stage = "oauth_token",
                status = status.as_u16(),
                oauth_error = %failure.error,
                "Microsoft OAuth 返回业务错误"
            );
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
        status => {
            tracing::warn!(
                stage = "oauth_token",
                status = status.as_u16(),
                "Microsoft OAuth 状态码异常"
            );
            Err(status_error(status))
        }
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
    stage: &'static str,
) -> Result<T, AuthError> {
    let response = send_with_retry(stage, true, || {
        Ok(post_json(client, endpoint, body)?.header("x-xbl-contract-version", "1"))
    })
    .await?;
    match response.status() {
        StatusCode::OK => parse_json(&response_body(response).await?),
        status @ (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
            let bytes = response_body(response).await?;
            let code = serde_json::from_slice::<XboxFailure>(&bytes)
                .ok()
                .map(|failure| failure.code);
            tracing::warn!(
                stage,
                status = status.as_u16(),
                xerr = ?code,
                "Xbox 认证拒绝请求"
            );
            Err(match code {
                Some(2148916233) => AuthError::XboxProfileRequired,
                Some(2148916227) => AuthError::XboxAccountBanned,
                Some(2148916229 | 2148916235) => AuthError::XboxRegionBlocked,
                Some(2148916234 | 2148916236 | 2148916237 | 2148916238) => {
                    AuthError::XboxChildAccount
                }
                _ => status_error(status),
            })
        }
        status => {
            tracing::warn!(stage, status = status.as_u16(), "Xbox 认证状态码异常");
            Err(status_error(status))
        }
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
    #[serde(default)]
    capes: Vec<ProfileCape>,
}

#[derive(Deserialize)]
struct ProfileSkin {
    state: String,
    variant: String,
    url: String,
}

#[derive(Deserialize)]
struct ProfileCape {
    id: String,
    state: String,
    url: String,
    #[serde(default)]
    alias: String,
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
    let capes = profile
        .capes
        .into_iter()
        .filter_map(|cape| {
            let valid_id = !cape.id.is_empty()
                && cape.id.len() <= 64
                && cape
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            let url = official_skin_url(&cape.url)?;
            if !valid_id {
                return None;
            }
            let alias = if cape.alias.trim().is_empty() {
                "Cape".to_owned()
            } else {
                cape.alias.trim().chars().take(64).collect()
            };
            Some(MinecraftCape {
                id: cape.id,
                alias,
                url,
                active: cape.state == "ACTIVE",
            })
        })
        .collect();
    Ok(MinecraftProfile {
        uuid,
        name: profile.name,
        skin_url,
        slim,
        capes,
    })
}

fn official_skin_url(input: &str) -> Option<String> {
    let mut url = Url::parse(input).ok()?;
    let texture_id = url.path().strip_prefix("/texture/")?;
    let valid = matches!(url.scheme(), "https" | "http")
        && url.host_str() == Some("textures.minecraft.net")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (1..=128).contains(&texture_id.len())
        && texture_id.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !valid {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.to_string())
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

fn transport_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_redirect() {
        "redirect"
    } else {
        "transport"
    }
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
        StatusCode::TOO_MANY_REQUESTS => AuthError::RateLimited,
        status if status.is_server_error() => AuthError::Unavailable,
        _ => AuthError::InvalidResponse,
    }
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, AuthError> {
    serde_json::from_slice(body).map_err(|_| AuthError::InvalidResponse)
}

fn post_json(client: &Client, endpoint: &str, json: &Value) -> Result<RequestBuilder, AuthError> {
    post_json_method(client.post(endpoint), json)
}

fn post_json_method(builder: RequestBuilder, json: &Value) -> Result<RequestBuilder, AuthError> {
    let body = serde_json::to_vec(json).map_err(|_| AuthError::InvalidResponse)?;
    Ok(builder.header(CONTENT_TYPE, "application/json").body(body))
}

async fn send_once(
    stage: &'static str,
    build: impl Fn() -> Result<RequestBuilder, AuthError>,
) -> Result<Response, AuthError> {
    send_with_retry(stage, false, build).await
}

async fn send_with_retry(
    stage: &'static str,
    retry: bool,
    build: impl Fn() -> Result<RequestBuilder, AuthError>,
) -> Result<Response, AuthError> {
    let mut attempt = 0;
    loop {
        let req = build()?.header(ACCEPT, "application/json");
        match req.send().await {
            Ok(response) => {
                if retry
                    && matches!(
                        response.status(),
                        StatusCode::TOO_MANY_REQUESTS
                            | StatusCode::INTERNAL_SERVER_ERROR
                            | StatusCode::BAD_GATEWAY
                            | StatusCode::SERVICE_UNAVAILABLE
                            | StatusCode::GATEWAY_TIMEOUT
                    )
                    && let Some(delay) = RETRY_DELAYS_MS.get(attempt)
                {
                    tracing::warn!(
                        stage,
                        status = response.status().as_u16(),
                        attempt,
                        "官方认证接口返回临时状态，短暂退避后重试"
                    );
                    attempt += 1;
                    time::sleep(Duration::from_millis(*delay)).await;
                    continue;
                }
                return Ok(response);
            }
            Err(error) => {
                let retryable = retry && (error.is_connect() || error.is_timeout());
                if retryable && let Some(delay) = RETRY_DELAYS_MS.get(attempt) {
                    tracing::warn!(
                        stage,
                        kind = transport_kind(&error),
                        attempt,
                        "官方认证接口网络波动，短暂退避后重试"
                    );
                    attempt += 1;
                    time::sleep(Duration::from_millis(*delay)).await;
                    continue;
                }
                tracing::warn!(stage, kind = transport_kind(&error), "官方认证接口请求失败");
                return Err(network_error(error));
            }
        }
    }
}

async fn official_response(
    stage: &'static str,
    retry: bool,
    build: impl Fn() -> Result<RequestBuilder, AuthError>,
    profile: bool,
) -> Result<Vec<u8>, AuthError> {
    let response = send_with_retry(stage, retry, build).await?;
    match response.status() {
        StatusCode::OK => response_body(response).await,
        StatusCode::NOT_FOUND if profile => Err(AuthError::NoJavaProfile),
        status => {
            tracing::warn!(
                stage,
                status = status.as_u16(),
                "Mojang/Xbox 官方接口返回错误状态"
            );
            Err(status_error(status))
        }
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
