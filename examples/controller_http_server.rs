//! Local GET-only fixture for examples/controllers_http.json; not a production server.
use std::{error::Error, io};

use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let address = args.next().unwrap_or_else(|| "127.0.0.1:8080".into());
    let max_requests = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    let listener = TcpListener::bind(&address).await?;
    // Port 0 selects an available port; print its actual address for --var base_url.
    println!(
        "{}",
        json!({"event":"listening", "base_url":format!("http://{}", listener.local_addr()?)})
    );
    let mut handled = 0;
    while max_requests.is_none_or(|limit| handled < limit) {
        let (stream, _) = tokio::select! {
            connection = listener.accept() => connection?,
            signal = tokio::signal::ctrl_c() => { signal?; break; }
        };
        timeout(Duration::from_secs(5), respond(stream)).await??;
        handled += 1;
    }
    println!("{}", json!({"event":"server_finished", "requests":handled}));
    Ok(())
}

async fn respond(mut stream: TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete HTTP request",
            ));
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers exceed 8 KiB",
            ));
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut parts = text.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let index = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("index="));
    let (status, reason, body) = match (method, path) {
        ("GET", "/status") => (
            200,
            "OK",
            json!({"code":0, "data":{"ready":true}}).to_string(),
        ),
        ("GET", "/poll") => (
            200,
            "OK",
            json!({"code":0, "data":{"index":index}}).to_string(),
        ),
        ("GET", "/xml") => (
            200,
            "OK",
            "<response><code>0</code><message>ready</message></response>".into(),
        ),
        ("GET", "/text") => (200, "OK", "service ready\nstatus=ok".into()),
        (_, "/must-not-run") => (
            500,
            "Internal Server Error",
            json!({"code":1, "message":"IF branch must be skipped"}).to_string(),
        ),
        _ => (
            404,
            "Not Found",
            json!({"code":1, "message":"unknown route"}).to_string(),
        ),
    };
    let content_type = match path {
        "/xml" => "application/xml",
        "/text" => "text/plain; charset=utf-8",
        _ => "application/json",
    };
    stream.write_all(format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    ).as_bytes()).await?;
    stream.shutdown().await?;
    println!(
        "{}",
        json!({"event":"request", "method":method, "path":path, "index":index, "status":status})
    );
    Ok(())
}
