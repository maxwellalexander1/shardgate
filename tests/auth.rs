use std::process::{Child, Command};
use std::time::Duration;

fn curl_status(port: u16, token: Option<&str>) -> (u16, String) {
    let url = format!("http://127.0.0.1:{}/check", port);
    let mut cmd = Command::new("curl");
    cmd.args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url]);
    if let Some(t) = token {
        cmd.args(["-H", &format!("Bearer {}", t)]);
        // curl needs header flag order: -H before url is fine anywhere
        // Rebuild correctly:
    }
    // Rebuild with correct order: curl -s -o /dev/null -w CODE [-H Bearer] url
    let mut cmd = Command::new("curl");
    cmd.arg("-s")
        .arg("-o")
        .arg("/dev/null")
        .arg("-w")
        .arg("%{http_code}");
    if let Some(t) = token {
        cmd.arg("-H").arg(format!("Authorization: Bearer {}", t));
    }
    cmd.arg(&url);
    let out = cmd.output().expect("curl");
    let code: u16 = String::from_utf8_lossy(&out.stdout).parse().unwrap_or(0);
    (code, String::new())
}

fn curl_body(port: u16, token: &str) -> (u16, String) {
    let url = format!("http://127.0.0.1:{}/check", port);
    let out = Command::new("curl")
        .args([
            "-s",
            "-w",
            "\n%{http_code}",
            "-H",
            &format!("Authorization: Bearer {}", token),
            &url,
        ])
        .output()
        .expect("curl");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut parts: Vec<&str> = text.rsplitn(2, '\n').collect();
    parts.reverse();
    if parts.len() == 2 {
        (parts[1].parse().unwrap_or(0), parts[0].to_string())
    } else {
        (0, text)
    }
}

fn wait_health(port: u16) {
    for _ in 0..50 {
        let out = Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                &format!("http://127.0.0.1:{}/healthz", port),
            ])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stdout) == "200" {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server did not start");
}

fn spawn(port: u16) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_shardgate-server"));
    cmd.env("PORT", port.to_string());
    cmd.env("SHARDGATE_CAPACITY", "2");
    cmd.env("SHARDGATE_REFILL_RATE", "0");
    cmd.env("SHARDGATE_TOKENS", "alice:secret1,bob:secret2");
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().expect("server")
}

#[test]
fn auth_required_and_isolated() {
    let port = 18081;
    let mut child = spawn(port);
    wait_health(port);

    // No token -> 401, never touches limiter.
    let (code, _) = curl_status(port, None);
    assert_eq!(code, 401, "no token must be 401");

    // Bad token -> 401.
    let (code, _) = curl_status(port, Some("wrong"));
    assert_eq!(code, 401, "bad token must be 401");

    // Alice: 2 allowed (capacity 2, refill 0), 3rd denied.
    let (c1, _) = curl_body(port, "secret1");
    let (c2, _) = curl_body(port, "secret1");
    let (c3, _) = curl_body(port, "secret1");
    assert_eq!((c1, c2, c3), (200, 200, 429), "alice bucket cap 2");

    // Bob independent: still gets 2.
    let (b1, body) = curl_body(port, "secret2");
    let (b2, _) = curl_body(port, "secret2");
    let (b3, _) = curl_body(port, "secret2");
    assert_eq!((b1, b2, b3), (200, 200, 429), "bob independent bucket");
    assert!(
        body.contains("bob"),
        "response key is identity, got {}",
        body
    );

    child.kill().ok();
}
