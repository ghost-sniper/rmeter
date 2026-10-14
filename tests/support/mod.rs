use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub struct TempScript(pub PathBuf);
impl TempScript {
    pub fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rmeter-script-{}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, source).unwrap();
        Self(path)
    }
}
impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn script(url: &str) -> Value {
    json!({"version": 1, "name": "local test", "groups": [{
        "name": "users", "children": [{
            "type": "http_request", "name": "status", "url": url,
            "assertions": [{"name": "code", "path": "$.code",
                "matcher": {"type": "equals", "expected": 0}}]
        }]
    }]})
}

pub async fn server(body: &'static str, requests: usize) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/status", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut recorded = Vec::new();
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0, "request ended early");
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            recorded.push(String::from_utf8(request).unwrap());
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        recorded
    });
    (url, task)
}
