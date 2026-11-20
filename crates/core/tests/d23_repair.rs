use std::path::Path;

use launcher_core::config::DownloadSource;
use launcher_core::install::LoaderContext;
use launcher_core::install::setup::repair_version;
use launcher_core::net::Downloader;
use launcher_core::net::sha1_file;
use launcher_core::paths::GameDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn repairs_same_size_corrupt_client_and_leaves_instance_files_alone() {
    let root = tempfile::tempdir().unwrap();
    let game = GameDir::new(root.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixed = b"verified-test-client";
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            fixed.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(fixed).await.unwrap();
    });
    let payload = root.path().join("expected.bin");
    std::fs::write(&payload, fixed).unwrap();
    let hash = sha1_file(&payload).await.unwrap();
    let version = "1.21.1";
    let meta = game.version_json(version).unwrap();
    std::fs::create_dir_all(meta.parent().unwrap()).unwrap();
    std::fs::write(
        meta,
        serde_json::json!({
            "id": version,
            "mainClass": "net.minecraft.client.main.Main",
            "downloads": { "client": {
                "url": format!("http://{address}/client.jar"),
                "sha1": hash,
                "size": fixed.len()
            } },
        })
        .to_string(),
    )
    .unwrap();
    let jar = game.version_jar(version).unwrap();
    std::fs::write(&jar, vec![b'!'; fixed.len()]).unwrap();
    let sentinels = [
        "instances/d23-fixture/saves/Test/level.dat",
        "instances/d23-fixture/mods/test.jar",
        "instances/d23-fixture/options.txt",
    ];
    for name in sentinels {
        let file = root.path().join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, name.as_bytes()).unwrap();
    }
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let downloader = Downloader::new(client.clone(), 1);
    let cancel = CancellationToken::new();
    let ctx = LoaderContext {
        game: &game,
        client: &client,
        downloader: &downloader,
        source: DownloadSource::OfficialOnly,
        meta_source: DownloadSource::OfficialOnly,
        java: Path::new(""),
        work_dir: root.path(),
        cancel: &cancel,
    };
    repair_version(&ctx, version, None, version, |_| {})
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(std::fs::read(&jar).unwrap(), fixed);
    for name in sentinels {
        assert_eq!(
            std::fs::read(root.path().join(name)).unwrap(),
            name.as_bytes()
        );
    }
    // 服务器已退出，完整文件仍能离线校验。
    repair_version(&ctx, version, None, version, |_| {})
        .await
        .unwrap();
    cancel.cancel();
    assert!(
        repair_version(&ctx, version, None, version, |_| {})
            .await
            .is_err()
    );
}
