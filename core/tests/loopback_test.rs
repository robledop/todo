use std::time::Duration;

use outlook_tasks_core::auth::LoopbackServer;

#[tokio::test]
async fn loopback_captures_code_for_matching_state() {
    let server = LoopbackServer::bind().unwrap();
    let redirect = server.redirect_url();
    assert!(redirect.starts_with("http://localhost:"));

    // The server blocks on recv inside spawn_blocking; drive it concurrently.
    let handle =
        tokio::spawn(async move { server.wait_for_code("xyz".into(), Duration::from_secs(5)).await });

    // Simulate the browser hitting the loopback redirect.
    let _ = reqwest::get(format!("{redirect}?code=abc&state=xyz")).await.unwrap();

    let params = handle.await.unwrap().unwrap();
    assert_eq!(params.code, "abc");
    assert_eq!(params.state, "xyz");
}

#[tokio::test]
async fn loopback_ignores_wrong_state_then_accepts_match() {
    let server = LoopbackServer::bind().unwrap();
    let redirect = server.redirect_url();
    let handle =
        tokio::spawn(async move { server.wait_for_code("good".into(), Duration::from_secs(5)).await });

    // A forged/wrong-state request must NOT end sign-in.
    let _ = reqwest::get(format!("{redirect}?code=evil&state=bad")).await.unwrap();
    // The genuine redirect then succeeds.
    let _ = reqwest::get(format!("{redirect}?code=real&state=good")).await.unwrap();

    let params = handle.await.unwrap().unwrap();
    assert_eq!(params.code, "real");
}

#[tokio::test]
async fn loopback_ignores_non_get_and_wrong_path() {
    let server = LoopbackServer::bind().unwrap();
    let redirect = server.redirect_url();
    let handle =
        tokio::spawn(async move { server.wait_for_code("good".into(), Duration::from_secs(5)).await });

    // A POST (even with the right state) is not our callback shape: ignored.
    let _ = reqwest::Client::new()
        .post(format!("{redirect}?code=x&state=good"))
        .send()
        .await
        .unwrap();
    // A GET to a different path is ignored.
    let _ = reqwest::get(format!("{redirect}other?code=x&state=good")).await.unwrap();
    // The genuine GET to "/" then succeeds.
    let _ = reqwest::get(format!("{redirect}?code=real&state=good")).await.unwrap();

    let params = handle.await.unwrap().unwrap();
    assert_eq!(params.code, "real");
}

#[tokio::test]
async fn loopback_times_out_without_redirect() {
    let server = LoopbackServer::bind().unwrap();
    let err = server.wait_for_code("s".into(), Duration::from_millis(150)).await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::AuthError::Protocol(_)));
}
