use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::Duration;
use tungstenite::{client, Message};

pub fn send_ws_command(command: serde_json::Value) {
    thread::spawn(move || {
        tracing::debug!("Connecting to WebSocket at ws://127.0.0.1:9002 ...");
        let addr: SocketAddr = "127.0.0.1:9002".parse().unwrap();
        let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("Failed to connect to WebSocket TCP socket: {}", e);
                return;
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        match client("ws://127.0.0.1:9002", stream) {
            Ok((mut socket, _)) => {
                tracing::debug!("Connected to WebSocket.");
                let msg = command.to_string();
                if let Err(e) = socket.send(Message::Text(msg)) {
                    tracing::debug!("WebSocket write error: {}", e);
                    return;
                }
                match socket.read() {
                    Ok(msg) => tracing::debug!("WebSocket received: {}", msg),
                    Err(e) => tracing::debug!("WebSocket read error: {}", e),
                }
                let _ = socket.close(None);
            }
            Err(e) => tracing::debug!("Failed to handshake WebSocket: {}", e),
        }
    });
}
