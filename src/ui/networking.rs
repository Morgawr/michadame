use std::thread;
use tungstenite::{connect, Message};
use url::Url;

pub fn send_ws_command(command: serde_json::Value) {
    thread::spawn(move || {
        println!("Connecting to WebSocket at ws://127.0.0.1:9002 ...");
        match connect(Url::parse("ws://127.0.0.1:9002").unwrap()) {
            Ok((mut socket, _)) => {
                println!("Connected to WebSocket.");
                let msg = command.to_string();
                if let Err(e) = socket.send(Message::Text(msg)) {
                    println!("WebSocket write error: {}", e);
                    return;
                }
                match socket.read() {
                    Ok(msg) => println!("WebSocket received: {}", msg),
                    Err(e) => println!("WebSocket read error: {}", e),
                }
                let _ = socket.close(None);
            }
            Err(e) => println!("Failed to connect to WebSocket: {}", e),
        }
    });
}
