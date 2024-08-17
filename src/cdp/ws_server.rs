use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};

#[derive(Debug, Clone)]
pub struct WebSocketServer {
    clients: Arc<Mutex<HashMap<usize, mpsc::UnboundedSender<Message>>>>,
    pub addr: Arc<String>,
    pub callback: Arc<Mutex<Option<CallbackWrapper>>>,
}

pub struct CallbackWrapper(pub Arc<dyn Fn(String) + Send + Sync>);

impl fmt::Debug for CallbackWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallbackWrapper")
    }
}

impl WebSocketServer {
    pub async fn new(addr: String) -> Arc<Self> {
        let server = Arc::new(Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            addr: Arc::new(addr.clone()),
            callback: Arc::new(Mutex::new(None)),
        });

        let server_clone = Arc::clone(&server);
        tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await.expect("Failed to bind");
            let mut id_counter = 0;

            while let Ok((stream, _)) = listener.accept().await {
                let server_clone = Arc::clone(&server_clone);
                id_counter += 1;
                let client_id = id_counter;

                tokio::spawn(async move {
                    let ws_stream = accept_async(stream).await.expect("Failed to accept");
                    let (tx, rx) = mpsc::unbounded_channel();
                    server_clone.clients.lock().await.insert(client_id, tx);

                    let callback = server_clone.callback.clone();
                    WebSocketServer::handle_client(ws_stream, rx, callback).await;

                    // Remove client after it disconnects
                    server_clone.clients.lock().await.remove(&client_id);
                });
            }
        });

        server
    }

    async fn handle_client(
        mut ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        mut rx: mpsc::UnboundedReceiver<Message>,
        callback: Arc<Mutex<Option<CallbackWrapper>>>,
    ) {
        loop {
            tokio::select! {
                Some(msg) = ws_stream.next() => {
                    if let Ok(Message::Text(text)) = msg {
                        if let Some(callback) = callback.lock().await.as_ref() {
                            callback.0(text);
                        }
                    }
                }
                Some(msg) = rx.recv() => {
                    if ws_stream.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    pub async fn send(&self, message: Value) -> Result<(), String> {
        let clients = self.clients.lock().await;
        if clients.is_empty() {
            return Err("No clients connected".to_string());
        }
        for client in clients.values() {
            let _ = client.send(Message::Text(message.to_string()));
        }
        Ok(())
    }

    pub fn addr(&self) -> String {
        self.addr.clone().to_string()
    }
}
