#![allow(clippy::unwrap_used)]

use super::*;
use tokio::io::{AsyncBufReadExt, BufReader};

const TEST_CLIENT_ID: &str = "e2f8ba33-0b03-4e3d-924f-e358fec1cb1d";

#[tokio::test]
async fn browser_uses_pkce_and_a_one_time_loopback_form_post() {
    let pending = start_login(TEST_CLIENT_ID, &LoginOptions::default())
        .await
        .unwrap();
    let url = pending.url();
    assert_eq!(url.host_str(), Some("login.microsoftonline.com"));
    assert_eq!(url.path(), "/consumers/oauth2/v2.0/authorize");
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["response_mode"], "form_post");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(
        params["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.expose()))
    );
    assert!(!url.as_str().contains(pending.verifier.expose()));
    assert_eq!(params["redirect_uri"], pending.redirect_uri);
    let state = params["state"].to_string();
    let addr = pending.listener.local_addr().unwrap();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
    let mut favicon = TcpStream::connect(addr).await.unwrap();
    favicon
        .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut forged = TcpStream::connect(addr).await.unwrap();
    let mut correct = TcpStream::connect(addr).await.unwrap();
    send_form(&mut forged, addr.port(), "state=wrong&code=forged").await;
    send_form(
        &mut correct,
        addr.port(),
        &format!("state={state}&code=test-auth-code"),
    )
    .await;
    let code = wait_for_code(&pending).await.unwrap();
    drop(pending);
    assert_eq!(code.expose(), "test-auth-code");
    let mut fav_resp = Vec::new();
    favicon.read_to_end(&mut fav_resp).await.unwrap();
    assert!(
        std::str::from_utf8(&fav_resp)
            .unwrap()
            .starts_with("HTTP/1.1 204")
    );
    let mut response = Vec::new();
    correct.read_to_end(&mut response).await.unwrap();
    let response = std::str::from_utf8(&response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("text/html; charset=utf-8"));
    assert!(response.contains("Referrer-Policy: no-referrer"));
    assert!(!response.contains("test-auth-code"));
    assert!(!response.contains(&state));
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn cancelled_login_closes_listener_without_connecting_to_microsoft() {
    let pending = start_login(TEST_CLIENT_ID, &LoginOptions::default())
        .await
        .unwrap();
    let addr = pending.listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        pending.finish(&cancel).await,
        Err(AuthError::Cancelled)
    ));
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn callback_rejects_duplicate_state_and_code_and_rejects_wrong_host() {
    let pending = start_login(TEST_CLIENT_ID, &LoginOptions::default())
        .await
        .unwrap();
    let addr = pending.listener.local_addr().unwrap();
    let state = pending.state.expose();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"POST / HTTP/1.1\r\nHost: example.test\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 1\r\n\r\na").await.unwrap();
    let mut duplicate = TcpStream::connect(addr).await.unwrap();
    send_form(
        &mut duplicate,
        addr.port(),
        &format!("state={state}&state={state}&code=first"),
    )
    .await;
    let mut doubled_code = TcpStream::connect(addr).await.unwrap();
    send_form(
        &mut doubled_code,
        addr.port(),
        &format!("state={state}&code=first&code=second"),
    )
    .await;
    let mut denied = TcpStream::connect(addr).await.unwrap();
    send_form(
        &mut denied,
        addr.port(),
        &format!("state={state}&error=access_denied&error_description=do-not-show"),
    )
    .await;
    assert!(matches!(
        wait_for_code(&pending).await,
        Err(AuthError::Denied)
    ));
}

async fn send_form(stream: &mut TcpStream, port: u16, body: &str) {
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
}

#[test]
fn accepts_only_recognized_java_entitlements_and_well_formed_profile() {
    let good = br#"{"items":[{"name":"game_minecraft"}]}"#;
    assert_eq!(check_java_license(good), Ok(()));
    for unowned in [
        br#"{"items":[]}"#.as_slice(),
        br#"{"items":[{"name":"game_minecraft_bedrock"}]}"#,
        br#"{"items":[{"name":"product_minecraft_trial"}]}"#,
    ] {
        assert_eq!(check_java_license(unowned), Err(AuthError::NoJavaLicense));
    }
    assert_eq!(
        check_java_license(br#"{}"#),
        Err(AuthError::InvalidResponse)
    );
    assert_eq!(
        read_java_profile(br#"{"id":"00000000000000000000000000000000","name":"Alex"}"#).err(),
        Some(AuthError::InvalidResponse)
    );
    assert_eq!(
        read_java_profile(br#"{"id":"123456789abc4def8000000000000000","name":"--accessToken"}"#)
            .err(),
        Some(AuthError::InvalidResponse)
    );
    let profile = read_java_profile(br#"{"id":"123456789abc4def8000000000000000","name":"Alex","skins":[{"state":"ACTIVE","variant":"SLIM","url":"https://textures.minecraft.net/texture/abcd"}]}"#).unwrap();
    assert_eq!(profile.name, "Alex");
    assert_eq!(
        profile.uuid.simple().to_string(),
        "123456789abc4def8000000000000000"
    );
    assert!(profile.slim);
    assert_eq!(
        profile.skin_url.as_deref(),
        Some("https://textures.minecraft.net/texture/abcd")
    );
    assert!(read_java_profile(br#"{"id":"123456789abc4def8000000000000000","name":"Alex","skins":[{"state":"ACTIVE","variant":"SLIM","url":"https://textures.minecraft.net.evil.example/texture/abcd"}]}"#).unwrap().skin_url.is_none());

    let caped = read_java_profile(br#"{"id":"123456789abc4def8000000000000000","name":"Alex","skins":[{"state":"ACTIVE","variant":"CLASSIC","url":"http://textures.minecraft.net/texture/1122aabb"}],"capes":[{"id":"migrator-1","state":"ACTIVE","url":"http://textures.minecraft.net/texture/cafe","alias":"Migrator"},{"id":"evil","state":"INACTIVE","url":"https://evil.example/texture/1","alias":"Evil"}]}"#).unwrap();
    assert_eq!(
        caped.skin_url.as_deref(),
        Some("https://textures.minecraft.net/texture/1122aabb")
    );
    assert_eq!(caped.capes.len(), 1);
    assert_eq!(caped.capes[0].id, "migrator-1");
    assert_eq!(caped.capes[0].alias, "Migrator");
    assert_eq!(
        caped.capes[0].url,
        "https://textures.minecraft.net/texture/cafe"
    );
    assert!(caped.capes[0].active);
}

#[test]
fn builds_rfc7578_skin_multipart_body_and_localized_html() {
    let (boundary, body) = build_skin_multipart(b"PNGDATA", true).unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains(&format!("--{boundary}")));
    assert!(text.contains("name=\"variant\"\r\n\r\nslim"));
    assert!(text.contains("filename=\"skin.png\""));
    assert!(text.contains("PNGDATA"));

    let en_html = branded_callback_html("en", HtmlOutcome::Success);
    assert!(en_html.contains("<html lang=\"en\">"));
    assert!(en_html.contains("Lumi Launcher"));
}

#[test]
fn accepts_only_nonzero_application_uuids() {
    assert!(valid_client_id(Some(TEST_CLIENT_ID)).is_some());
    for id in [
        "",
        "placeholder",
        "00000000-0000-0000-0000-000000000000",
        " e2f8ba33-0b03-4e3d-924f-e358fec1cb1d",
    ] {
        assert!(valid_client_id(Some(id)).is_none());
    }
    assert!(valid_client_id(None).is_none());
}

const OAUTH: &str = r#"{"access_token":"oauth-test-token","refresh_token":"rotated-test-refresh","token_type":"Bearer","expires_in":3600}"#;
const XBOX_USER: &str = r#"{"Token":"xbox-test-token","DisplayClaims":{"xui":[{"uhs":"111111"}]}}"#;
const XSTS: &str = r#"{"Token":"xsts-test-token","DisplayClaims":{"xui":[{"uhs":"222222"}]}}"#;
const GAME: &str = r#"{"access_token":"game-test-token","token_type":"Bearer","expires_in":86400}"#;
const OWNED: &str = r#"{"items":[{"name":"game_minecraft"}]}"#;
const PROFILE: &str = r#"{"id":"123456789abc4def8000000000000000","name":"Alex"}"#;

#[tokio::test]
async fn verifies_each_handoff_and_uses_the_same_game_token_for_both_decisions() {
    let (outcome, requests) = mock_exchange(&[
        ("200 OK", OAUTH),
        ("200 OK", XBOX_USER),
        ("200 OK", XSTS),
        ("200 OK", GAME),
        ("200 OK", OWNED),
        ("200 OK", PROFILE),
    ])
    .await;
    let verified = outcome.unwrap();
    assert_eq!(verified.game_token().expose(), "game-test-token");
    assert_eq!(verified.refresh_token().expose(), "rotated-test-refresh");
    assert_eq!(verified.profile().name, "Alex");
    assert!(verified.valid_until() > Instant::now());
    assert!(verified.valid_until() <= Instant::now() + VERIFICATION_WINDOW);
    assert!(!format!("{:?}", verified.refresh_token()).contains("rotated-test-refresh"));

    assert_eq!(
        requests
            .iter()
            .map(|request| request.line.as_str())
            .collect::<Vec<_>>(),
        [
            "POST /token HTTP/1.1",
            "POST /user/authenticate HTTP/1.1",
            "POST /xsts/authorize HTTP/1.1",
            "POST /authentication/login_with_xbox HTTP/1.1",
            "GET /entitlements/mcstore HTTP/1.1",
            "GET /minecraft/profile HTTP/1.1",
        ]
    );
    assert!(requests[0].body.contains("grant_type=refresh_token"));
    assert!(
        requests[0]
            .body
            .contains("refresh_token=test-refresh%2B%3D")
    );
    let json: Vec<Value> = requests[1..4]
        .iter()
        .map(|request| serde_json::from_str(&request.body).unwrap())
        .collect();
    assert_eq!(json[0]["Properties"]["RpsTicket"], "d=oauth-test-token");
    assert_eq!(
        json[1]["Properties"]["UserTokens"],
        json!(["xbox-test-token"])
    );
    assert_eq!(json[1]["RelyingParty"], "rp://api.minecraftservices.com/");
    assert_eq!(json[2]["identityToken"], "XBL3.0 x=222222;xsts-test-token");
    assert!(
        requests[..4]
            .iter()
            .all(|request| request.authorization.is_none())
    );
    for request in &requests[4..] {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer game-test-token")
        );
        assert!(request.body.is_empty());
    }
}

#[tokio::test]
async fn refuses_unknown_license_missing_profile_unauthorized_and_restricted_accounts() {
    let before_ownership = [
        ("200 OK", OAUTH),
        ("200 OK", XBOX_USER),
        ("200 OK", XSTS),
        ("200 OK", GAME),
    ];
    let no_license = [
        before_ownership.as_slice(),
        &[("200 OK", r#"{"items":[]}"#)],
    ]
    .concat();
    let (outcome, requests) = mock_exchange(&no_license).await;
    assert_eq!(outcome.err(), Some(AuthError::NoJavaLicense));
    assert_eq!(requests.len(), 5);
    let missing_profile = [
        before_ownership.as_slice(),
        &[("200 OK", OWNED), ("404 Not Found", "{}")],
    ]
    .concat();
    let (outcome, _) = mock_exchange(&missing_profile).await;
    assert_eq!(outcome.err(), Some(AuthError::NoJavaProfile));
    let forbidden = [
        before_ownership.as_slice(),
        &[("403 Forbidden", r#"{"error":"fake-secret"}"#)],
    ]
    .concat();
    let (outcome, _) = mock_exchange(&forbidden).await;
    let denied = outcome.err().unwrap();
    assert_eq!(denied, AuthError::AppAccessDenied);
    assert!(!format!("{denied} {denied:?}").contains("fake-secret"));
    let restricted = [
        ("200 OK", OAUTH),
        ("200 OK", XBOX_USER),
        (
            "401 Unauthorized",
            r#"{"XErr":2148916238,"Redirect":"https://other.example/fake-secret"}"#,
        ),
    ];
    let (outcome, _) = mock_exchange(&restricted).await;
    assert_eq!(outcome.err(), Some(AuthError::XboxChildAccount));
    let banned = [
        ("200 OK", OAUTH),
        ("200 OK", XBOX_USER),
        ("401 Unauthorized", r#"{"XErr":2148916227}"#),
    ];
    let (outcome, _) = mock_exchange(&banned).await;
    assert_eq!(outcome.err(), Some(AuthError::XboxAccountBanned));
    let revoked = [(
        "400 Bad Request",
        r#"{"error":"invalid_grant","error_description":"fake-secret"}"#,
    )];
    let (outcome, _) = mock_exchange(&revoked).await;
    assert_eq!(outcome.err(), Some(AuthError::ReloginRequired));
}

#[tokio::test]
async fn refuses_token_replay_to_http_redirects() {
    let (outcome, requests) =
        mock_exchange(&[("307 Temporary Redirect\r\nLocation: /unexpected", "{}")]).await;
    assert_eq!(outcome.err(), Some(AuthError::InvalidResponse));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].line, "POST /token HTTP/1.1");
}

struct RecordedRequest {
    line: String,
    authorization: Option<String>,
    body: String,
}

async fn mock_exchange(
    replies: &[(&'static str, &'static str)],
) -> (Result<VerifiedSession, AuthError>, Vec<RecordedRequest>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let endpoints = Endpoints {
        token: &format!("{base}/token"),
        xbox_user: &format!("{base}/user/authenticate"),
        xbox_authorize: &format!("{base}/xsts/authorize"),
        minecraft_login: &format!("{base}/authentication/login_with_xbox"),
        entitlements: &format!("{base}/entitlements/mcstore"),
        profile: &format!("{base}/minecraft/profile"),
        skins: &format!("{base}/minecraft/profile/skins"),
        skins_active: &format!("{base}/minecraft/profile/skins/active"),
        capes_active: &format!("{base}/minecraft/profile/capes/active"),
    };
    let (done, stop) = (CancellationToken::new(), CancellationToken::new());
    let mock = reply_to_requests(&listener, replies, &done);
    let client = auth_http_builder().no_proxy().build().unwrap();
    let renewal = Secret::new("test-refresh+=".to_owned());
    let form = [
        ("client_id", TEST_CLIENT_ID),
        ("grant_type", "refresh_token"),
        ("refresh_token", renewal.expose()),
    ];
    let exchange = async {
        let outcome = verify(&client, &endpoints, &form, Some(&renewal), &stop, &|_| {}).await;
        done.cancel();
        outcome
    };
    time::timeout(Duration::from_secs(5), async {
        tokio::join!(exchange, mock)
    })
    .await
    .unwrap()
}

async fn reply_to_requests(
    listener: &TcpListener,
    replies: &[(&str, &str)],
    done: &CancellationToken,
) -> Vec<RecordedRequest> {
    let mut requests = Vec::new();
    loop {
        let (stream, _) = tokio::select! {
            biased;
            () = done.cancelled() => return requests,
            connection = listener.accept() => connection.unwrap(),
        };
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let mut length = 0;
        let mut authorization = None;
        let mut headers = (&mut stream).lines();
        while let Some(header) = headers.next_line().await.unwrap() {
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_owned());
            }
        }
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        let (status, payload) = replies
            .get(requests.len())
            .copied()
            .unwrap_or(("500 Unexpected Request", "{}"));
        requests.push(RecordedRequest {
            line: line.trim_end().to_owned(),
            authorization,
            body: String::from_utf8(body).unwrap(),
        });
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        );
        stream
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();
    }
}
