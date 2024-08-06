use crate::cdp::cdp_protocol::{CdpProtocol, Domain, Parameters};
use crate::cdp::ws_server::{CallbackWrapper, WebSocketServer};
use anyhow::{anyhow, Context, Result};
use dotenv::dotenv;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::net::TcpListener;
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use super::cdp_protocol::Protocol;

static DEBUG: bool = false;
static CHROME_STDERR: bool = false;

pub type EventHandlerFn = dyn Fn(&Value) + Send + Sync;

struct DebuggableEventHandler {
    handler: Arc<EventHandlerFn>,
}

impl fmt::Debug for DebuggableEventHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EventHandler")
    }
}

impl Clone for DebuggableEventHandler {
    fn clone(&self) -> Self {
        DebuggableEventHandler {
            handler: Arc::clone(&self.handler),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CdpResponse {
    value: Value,
}

impl CdpResponse {
    pub fn new(value: Value) -> Self {
        CdpResponse { value }
    }

    pub fn get_i32(&self, field: &str) -> Result<i32> {
        if let Some(result) = self.value.get("result") {
            result
                .get(field)
                .with_context(|| format!("Field '{}' not found in response: {:?}", field, result))?
                .as_i64()
                .with_context(|| format!("Field '{}' is not an i64", field))?
                .try_into()
                .with_context(|| format!("Failed to convert '{}' to i32", field))
        } else {
            Err(anyhow!("Field '{}' not found, 'result' is empty.", field))
        }
    }

    pub fn get_result(&self) -> Value {
        self.value
            .get("result")
            .cloned()
            .unwrap_or(self.value.clone())
    }
}

impl Deref for CdpResponse {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CDP {
    cdp_host: String,
    request_id: Arc<AtomicU64>,
    pub cdp_ws_url: String,
    ws_transmit: mpsc::UnboundedSender<Message>,
    ws_receive:
        Arc<Mutex<mpsc::UnboundedReceiver<Result<Message, tokio_tungstenite::tungstenite::Error>>>>,
    keep_alive_interval: Duration,
    requests: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    event_handlers: Arc<Mutex<HashMap<String, DebuggableEventHandler>>>,
    pub root_node_id: Arc<Mutex<Option<i64>>>,
    pub ws_server: Arc<WebSocketServer>,
    protocol: Arc<Mutex<Protocol>>,
    mode: bool,
}

impl CDP {
    pub async fn new(mode: bool) -> Result<Arc<Self>> {
        dotenv().ok();
        let chrome_exe_path = env::var("CHROME_EXE_PATH")
            .expect("Missing CHROME_EXE_PATH='', add executable path to .env file.");
        let debugger_port = find_free_port()?;
        let debugger_host = format!("127.0.0.1:{}", debugger_port);
        let cdp_host = format!("http://{}", debugger_host);
        let keep_alive_interval = Duration::from_secs(30);

        // Start Chrome
        let exe_path = env::current_exe()?;
        let exe_dir = exe_path.parent().unwrap();
        let user_data_dir = exe_dir.join("tmp/browser/profile");
        let no_user_data = !user_data_dir.exists();
        let mut child = start_chrome(&user_data_dir, &chrome_exe_path, debugger_port).await?;

        // Initialize event_handlers
        let event_handlers = Arc::new(Mutex::new(HashMap::new()));

        // Create WebSocketServer
        let ws_server_port = find_free_port()?;
        let ws_server = WebSocketServer::new(format!("127.0.0.1:{}", ws_server_port)).await;

        // Create CDP instance
        let cdp = Arc::new(CDP {
            cdp_host: cdp_host.clone(),
            request_id: Arc::new(AtomicU64::new(1)),
            cdp_ws_url: String::new(),
            ws_transmit: mpsc::unbounded_channel().0,
            ws_receive: Arc::new(Mutex::new(mpsc::unbounded_channel().1)),
            keep_alive_interval,
            requests: Arc::new(Mutex::new(HashMap::new())),
            event_handlers,
            root_node_id: Arc::new(Mutex::new(None)),
            ws_server,
            protocol: Arc::new(Mutex::new(Protocol::new())),
            mode,
        });

        // Get WebSocket URL
        let response = reqwest::get(format!("{}/json/list", cdp_host))
            .await?
            .json::<Value>()
            .await?;
        let cdp_ws_url = response[0]["webSocketDebuggerUrl"]
            .as_str()
            .expect("Failed to get WebSocketDebuggerUrl")
            .to_string();

        // Establish WebSocket connection
        let (ws_stream, _) = connect_async(&cdp_ws_url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (write_tx, mut write_rx) = mpsc::unbounded_channel();
        let (_read_tx, read_rx) = mpsc::unbounded_channel();

        let ws_receive = Arc::new(Mutex::new(read_rx));
        let ping_tx = write_tx.clone();

        tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if ws_write.send(msg).await.is_err() {
                    error!("Failed to send message, closing WebSocket");
                    break;
                }
            }
        });

        // Update CDP instance with WebSocket URL and channels
        let cdp = Arc::new(CDP {
            cdp_ws_url,
            ws_transmit: write_tx,
            ws_receive: ws_receive.clone(),
            ..(*cdp).clone()
        });

        let cdp_clone = Arc::clone(&cdp);
        tokio::spawn(async move {
            while let Some(msg) = ws_read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if DEBUG {
                            debug!("WS:Received: {:?}", text.clone());
                        }
                        let resp = serde_json::from_str::<Value>(&text).unwrap();
                        let cdp_clone = Arc::clone(&cdp_clone);
                        tokio::spawn(async move {
                            if let Some(method) = resp.get("method").and_then(|m| m.as_str()) {
                                cdp_clone.handle_event(method, &resp).await
                            } else if let Some(req_id) = resp.get("id").and_then(|id| id.as_u64()) {
                                let tx = {
                                    let mut pending_requests = cdp_clone.requests.lock().await;
                                    pending_requests.remove(&req_id)
                                };
                                if let Some(tx) = tx {
                                    if DEBUG {
                                        debug!("WS:Sending response to request ID: {}", req_id);
                                    }
                                    let _ = tx.send(Ok(resp));
                                } else {
                                    if DEBUG {
                                        debug!("WS:No pending request for ID: {}", req_id);
                                    }
                                }
                            }
                        });
                    }
                    Ok(Message::Pong(_)) => {
                        // Do nothing for Pong messages
                    }
                    Ok(other) => {
                        warn!("Received non-text message: {:?}", other);
                    }
                    Err(e) => {
                        if DEBUG {
                            error!("Error receiving WebSocket message: {:?}", e);
                        }
                        break;
                    }
                }
            }
            if DEBUG {
                debug!("Exiting WebSocket read loop");
            }
        });

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(keep_alive_interval).await;
                if ping_tx.send(Message::Ping(vec![])).is_err() {
                    error!("Failed to send ping, WebSocket might be closed");
                    std::process::exit(1);
                }
            }
        });

        if no_user_data {
            warn!("Chrome profile not found, stopping to generate one..");
            tokio::time::sleep(Duration::from_secs(7)).await; // (!) might take longer to generate on slower computers
            let _ = cdp.send("Browser.close", None).await;
            std::process::exit(0);
        }

        let user_data_preferences = user_data_dir.join("Default").join("Preferences");
        if !cdp.check_for_preferences(&user_data_preferences).await? {
            info!("Search engine choice screen..");
            loop {
                if cdp.check_for_preferences(&user_data_preferences).await? {
                    info!("Search engine choice screen completed. Profile completed.");
                    std::process::exit(0);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        tokio::spawn(async move {
            loop {
                if let Ok(Some(status)) = child.try_wait() {
                    if DEBUG {
                        debug!("{:?}", status);
                    }
                    std::process::exit(0);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        if mode {
            // enable dom
            let _ = cdp
                .send("DOM.enable", Some(json!({"includewhitespace": "All"})))
                .await;

            // enable page
            let _ = cdp.send("Page.enable", None).await;

            // inject WS server handler script
            cdp.inject_ws_handler_script().await?;
        }

        // fetch protocol data
        let response = reqwest::get(format!("{}/json/protocol", cdp_host)).await?;
        let protocol: CdpProtocol = response.json().await?;
        {
            let mut protocol_guard = cdp.protocol.lock().await;
            *protocol_guard = protocol;
        }

        Ok(cdp)
    }

    async fn inject_ws_handler_script(&self) -> Result<()> {
        let ws_server_addr = self.ws_server.addr();
        let js = format!(
            r#"
            (function() {{
                const ws = new WebSocket('ws://{}');

                ws.onopen = function(event) {{
                    if (typeof window.wsHandlers !== 'undefined' && typeof window.wsHandlers.onOpen === 'function') {{
                        window.wsHandlers.onOpen(event);
                    }}
                }};

                ws.onmessage = function(event) {{
                    if (typeof window.wsHandlers !== 'undefined' && typeof window.wsHandlers.onMessage === 'function') {{
                        window.wsHandlers.onMessage(event);
                    }}
                }};

                ws.onerror = function(event) {{
                    if (typeof window.wsHandlers !== 'undefined' && typeof window.wsHandlers.onError === 'function') {{
                        window.wsHandlers.onError(event);
                    }}
                }};

                ws.onclose = function(event) {{
                    if (typeof window.wsHandlers !== 'undefined' && typeof window.wsHandlers.onClose === 'function') {{
                        window.wsHandlers.onClose(event);
                    }}
                }};

                window.sendToWs = function(message) {{
                    ws.send(JSON.stringify(message));
                }};

                window.setupWsHandlers = function(handlers) {{
                    window.wsHandlers = handlers;
                }};
            }})();
            "#,
            ws_server_addr
        );

        let _ = self
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                Some(json!({
                    "source": js,
                })),
            )
            .await;
        Ok(())
    }

    async fn check_for_preferences(&self, user_data_preferences: &Path) -> Result<bool> {
        let mut file = File::open(&user_data_preferences)
            .with_context(|| format!("Failed to open file: {:?}", user_data_preferences))
            .unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .with_context(|| format!("Failed to read file: {:?}", user_data_preferences))
            .unwrap();

        let preferences: Value = serde_json::from_str(&contents)
            .with_context(|| {
                format!(
                    "Failed to parse JSON from file: {:?}",
                    user_data_preferences
                )
            })
            .unwrap();

        if let Some(default_search_provider) = preferences.get("default_search_provider") {
            if default_search_provider
                .get("choice_screen_completion_timestamp")
                .is_some()
            {
                return Ok(true);
            } else {
                return Ok(false);
            }
        }
        Ok(false)
    }

    async fn handle_invalid_params_error(&self, method: &str) {
        let protocol_guard = self.protocol.lock().await;
        if let Some(domain) = protocol_guard
            .domains
            .iter()
            .find(|d| method.starts_with(&d.domain))
        {
            if let Some(command) = domain.commands.iter().find(|c| method.ends_with(&c.name)) {
                let formatted_params = if let Some(parameters) = &command.parameters {
                    parameters
                        .iter()
                        .map(|param| self.format_parameter(param))
                        .collect::<Vec<String>>()
                        .join(",\n")
                } else {
                    String::from("No parameters")
                };

                error!(
                    "Invalid parameters for method '{}'. Expected parameters inside serde_json::json!:\n{}",
                    method, formatted_params
                );
            } else {
                error!(
                    "Method '{}' not found in domain '{}'",
                    method, domain.domain
                );
            }
        } else {
            error!("Domain for method '{}' not found", method);
        }
    }

    fn format_parameter(&self, param: &Parameters) -> String {
        format!(
            "Parameters {{\n  name: \"{}\",\n  description: {:?},\n  experimental: {:?},\n  deprecated: {:?},\n  optional: {:?},\n  type_name: {:?},\n  ref_type_name: {:?},\n  enums: {:?}\n}}",
            param.name,
            param.description,
            param.experimental,
            param.deprecated,
            param.optional,
            param.type_name,
            param.ref_type_name,
            param.enums
        )
    }

    async fn handle_method_not_found_error(&self, method: &str) {
        let protocol_guard = self.protocol.lock().await;
        let parts: Vec<&str> = method.split('.').collect();

        if parts.len() == 2 {
            let (domain_name, command_name) = (parts[0], parts[1]);

            let domain_matches: Vec<&Domain> = protocol_guard
                .domains
                .iter()
                .filter(|domain| domain.domain.starts_with(domain_name))
                .collect();

            if domain_matches.is_empty() {
                error!(
                    "Domain '{}' wasn't found. Available domains: {:?}",
                    domain_name,
                    protocol_guard
                        .domains
                        .iter()
                        .map(|d| &d.domain)
                        .collect::<Vec<&String>>()
                );
            } else {
                let mut command_matches = vec![];
                for domain in &domain_matches {
                    for command in &domain.commands {
                        if command.name.starts_with(command_name) {
                            command_matches.push(format!("{}.{}", domain.domain, command.name));
                        }
                    }
                }
                if command_matches.is_empty() {
                    error!(
                        "Command '{}' wasn't found in domain '{}'. Available commands in domain '{}': {:?}",
                        command_name,
                        domain_name,
                        domain_name,
                        domain_matches
                            .iter()
                            .flat_map(|d| d.commands.iter().map(|c| &c.name))
                            .collect::<Vec<&String>>()
                    );
                } else {
                    error!(
                        "Command '{}' wasn't found. Did you mean one of these? {:?}",
                        method, command_matches
                    );
                }
            }
        } else {
            let domain_name = parts[0];
            let domain_matches: Vec<&Domain> = protocol_guard
                .domains
                .iter()
                .filter(|domain| domain.domain.starts_with(domain_name))
                .collect();

            if domain_matches.is_empty() {
                error!(
                    "No domain found starting with '{}'. Available domains: {:?}",
                    domain_name,
                    protocol_guard
                        .domains
                        .iter()
                        .map(|d| &d.domain)
                        .collect::<Vec<&String>>()
                );
            } else {
                error!(
                    "Invalid method format '{}'. Did you mean one of these domains? {:?}",
                    method,
                    domain_matches
                        .iter()
                        .map(|d| &d.domain)
                        .collect::<Vec<&String>>()
                );
            }
        }
    }

    pub async fn send(&self, method: &str, params: Option<Value>) -> Result<CdpResponse> {
        let reqid = self.request_id.fetch_add(1, Ordering::SeqCst);
        if DEBUG {
            warn!("Sending request: #{} @ {}", reqid, method);
        }

        let message = json!({
            "id": reqid,
            "method": method,
            "params": params.unwrap_or_default(),
        });

        let (tx, rx) = oneshot::channel();
        {
            let mut pending_requests = self.requests.lock().await;
            pending_requests.insert(reqid, tx);
        }

        if self
            .ws_transmit
            .send(Message::Text(message.to_string()))
            .is_err()
        {
            error!("WebSocket write channel is not available");
            return Err(anyhow::anyhow!("WebSocket write channel is not available"));
        }

        if DEBUG {
            debug!("Awaiting response for request: #{} @ {}", reqid, method);
        }

        let resp = rx.await??;
        if DEBUG {
            info!("Received response for request: #{} @ {}", reqid, method);
        }

        if resp.get("error").is_none() {
            Ok(CdpResponse::new(resp))
        } else {
            let error_code = resp["error"]["code"].as_i64().unwrap_or_default();
            match error_code {
                -32602 => {
                    self.handle_invalid_params_error(method).await;
                }
                -32601 => {
                    self.handle_method_not_found_error(method).await;
                }
                _ => {
                    error!("CDP error: {:?} {:?}", method, resp);
                }
            }

            Err(anyhow::anyhow!("CDP error: {:?}", resp))
        }
    }

    pub async fn update_root_node_id(&self) -> Result<()> {
        // refresh nodeid's with getDocument
        let document = self
            .send(
                "DOM.getDocument",
                Some(json!({
                    "depth": -1,
                    "pierce": true
                })),
            )
            .await?
            .get_result();

        // Extract root node ID
        let root_node_id = document["root"]["nodeId"]
            .as_i64()
            .context("Failed to get root nodeId")?;

        // Update root_node_id
        let mut root_node_id_guard = self.root_node_id.lock().await;
        *root_node_id_guard = Some(root_node_id);

        Ok(())
    }

    pub async fn handle_event(&self, method: &str, resp: &Value) {
        // Handle internal events first
        if self.mode {
            if method == "DOM.documentUpdated" {
                if let Err(e) = self.update_root_node_id().await {
                    error!("Failed to update root node ID: {:?}", e);
                }
            }
            if method == "Page.frameStoppedLoading" {
                if let Err(e) = self.update_root_node_id().await {
                    error!("Failed to update root node ID: {:?}", e);
                }
            }
        }

        // Invoke user handlers
        let handlers = self.event_handlers.lock().await;
        if let Some(handler) = handlers.get(method) {
            (handler.handler)(resp);
        }
    }

    #[allow(dead_code)]
    pub async fn register_event_handler<F>(&self, event_name: &str, handler: F)
    where
        F: Fn(&Value) + Send + Sync + 'static,
    {
        let handler = Arc::new(handler) as Arc<EventHandlerFn>;
        let mut handlers = self.event_handlers.lock().await;
        handlers.insert(event_name.to_string(), DebuggableEventHandler { handler });
    }

    #[allow(dead_code)]
    pub async fn unregister_event_handler(&self, event_name: &str) {
        self.event_handlers.lock().await.remove(event_name);
    }

    // WS server
    #[allow(dead_code)]
    pub fn ws_server_addr(&self) -> String {
        self.ws_server.addr.clone().to_string()
    }

    #[allow(dead_code)]
    pub async fn js_handle<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut cb = self.ws_server.callback.lock().await;
        *cb = Some(CallbackWrapper(Arc::new(callback)));
    }
}

async fn start_chrome(
    user_data_dir: &Path,
    chrome_exe_path: &str,
    debugger_port: u16,
) -> Result<tokio::process::Child> {
    let mut chrome_args = vec![
        format!("--remote-debugging-port={}", debugger_port),
        format!("--remote-allow-origins=http://127.0.0.1:{}", debugger_port),
        "--disable-extensions".to_string(),
        "--disable-popup-blocking".to_string(),
        "--disable-site-isolation-trials".to_string(),
        "--v=1".to_string(),
        "--disable-software-rasterizer".to_string(),
        "--no-default-browser-check".to_string(),
        "--no-first-run".to_string(),
        "--disable-default-apps".to_string(),
        "--disable-plugins-discovery".to_string(),
        "--incognito".to_string(),
        "--disable-features=Translate".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-background-timer-throttling".to_string(),
        format!("--user-data-dir={}", user_data_dir.to_str().unwrap()),
    ];

    if CHROME_STDERR {
        chrome_args.push("--enable-logging=stderr".to_string());
    }

    let child = Command::new(chrome_exe_path)
        .args(&chrome_args)
        .spawn()
        .context("Failed to start Chrome")?;

    Ok(child)
}

fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
