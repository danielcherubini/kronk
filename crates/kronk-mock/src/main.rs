use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::sleep;

static TOKEN_COUNT: AtomicUsize = AtomicUsize::new(0);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    
    let crash_after = args.iter().find(|a| *a == "--crash-after")
        .and_then(|a| a.split('=').nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let hang = args.iter().any(|a| *a == "--hang");
    let port = args.iter().find(|a| *a == "--port")
        .and_then(|a| a.parse().ok())
        .unwrap_or(8080);

    tracing::info!("kronk-mock starting (port={port}, crash_after={:?}, hang={})", crash_after, hang);

    let _output_handle = tokio::spawn(async move {
        if hang {
            tracing::info!("Mock backend: HANGING (no output)");
            loop {
                sleep(Duration::from_secs(60)).await;
            }
        } else {
            let mut count = 0;
            loop {
                sleep(Duration::from_millis(100)).await;
                count += 1;
                TOKEN_COUNT.store(count, Ordering::SeqCst);
                if crash_after > 0 && count >= crash_after {
                    tracing::error!("Mock backend: CRASHING after {} seconds", crash_after);
                    break;
                }
                println!("[MOCK] Generating token {}...", count);
                println!("[MOCK] Token: {{ \"id\": {}, \"content\": \"token_{}\" }}", count, count);
                println!();
                sleep(Duration::from_millis(50)).await;
            }
        }
    });

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
    tracing::info!("Health check server listening on http://127.0.0.1:{port}");

    loop {
        let (socket, addr) = listener.accept().unwrap();
        tracing::debug!("Health check from {}", addr);
        
        let _count = AtomicUsize::new(0);
        let _crash_after = crash_after;
        
        std::thread::spawn(move || {
            let mut socket = socket;
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf);
            
            let request = String::from_utf8_lossy(&buf);
            tracing::debug!("Request: {}", request.trim());
            
            let response = if request.contains("/health") {
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nOK"
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nNot Found"
            };
            
            socket.write_all(response.as_bytes()).ok();
        });
    }
}
