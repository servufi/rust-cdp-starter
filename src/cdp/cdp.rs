use crate::cdp::cdp_protocol::{CdpProtocol, Domain, Parameters};
use crate::cdp::ws_server::{CallbackWrapper, WebSocketServer};
use anyhow::{anyhow, Context, Result};
use dotenv::dotenv;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, warn};
use once_cell::sync::{Lazy, OnceCell};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fmt::{self};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::signal;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use super::cdp_protocol::Protocol;

static DEBUG: bool = false;
static CHROME_STDERR: bool = false;

pub type EventHandlerFn = dyn Fn(&Value) + Send + Sync;

pub struct DebuggableEventHandler {
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

static CDP_BROWSERS: Lazy<Arc<CdpBrowsers>> = Lazy::new(|| Arc::new(CdpBrowsers::new()));

#[derive(Debug, Clone)]
pub struct CdpBrowsers {
    pub profiles: Arc<Mutex<HashMap<String, BrowserInfo>>>,
    pub protocol: Arc<OnceCell<Protocol>>,
}

impl CdpBrowsers {
    pub fn new() -> Self {
        CdpBrowsers {
            profiles: Arc::new(Mutex::new(HashMap::new())),
            protocol: Arc::new(OnceCell::new()),
        }
    }

    pub async fn add_profile(
        &self,
        profile_name: String,
        debugger_id: String,
        port: u16,
        browser_context_id: String,
        initial_cdp: Arc<CDP>,
    ) {
        let mut profiles = self.profiles.lock().await;
        profiles.insert(
            profile_name,
            BrowserInfo {
                debugger_id,
                port,
                browser_context_id,
                initial_cdp,
            },
        );
    }

    pub async fn get_profile(&self, profile_name: &str) -> Option<BrowserInfo> {
        let profiles = self.profiles.lock().await;
        profiles.get(profile_name).cloned()
    }

    pub async fn get_profile_name_by_port(&self, port: u16) -> Option<String> {
        let profiles = self.profiles.lock().await;
        profiles.iter().find_map(|(profile_name, info)| {
            if info.port == port {
                Some(profile_name.clone())
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct BrowserInfo {
    pub debugger_id: String,
    pub port: u16,
    pub browser_context_id: String,
    pub initial_cdp: Arc<CDP>,
}

#[derive(Serialize, Debug)]
pub struct SendPayload {
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CdpResponse {
    value: Value,
}

impl CdpResponse {
    pub fn new(value: Value) -> Self {
        CdpResponse { value }
    }

    pub fn as_i32(&self, field: &str) -> Result<i32> {
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
pub struct Connection {
    pub id: String,
    pub request_id: Arc<AtomicU64>,
    pub ws_transmit: mpsc::UnboundedSender<Message>,
    pub ws_receive:
        Arc<Mutex<mpsc::UnboundedReceiver<Result<Message, tokio_tungstenite::tungstenite::Error>>>>,
    pub requests: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CDP {
    pub cdp_host: String,
    pub keep_alive_interval: Duration,
    pub event_handlers: Arc<Mutex<HashMap<String, DebuggableEventHandler>>>,
    pub ws_server: Arc<WebSocketServer>,
    pub connection: Arc<Connection>,
    pub params: Value,
    pub profile_name: String,
}

impl CDP {
    // New browser or tab
    pub async fn new(params: Option<Value>) -> Result<Arc<Self>> {
        // Set .env
        dotenv().ok();

        // Setup Ctrl+C handler on the first initialization of CDP
        CDP::setup_ctrlc_handler().await;

        // Get chrome.exe path
        let chrome_exe_path = env::var("CHROME_EXE_PATH")
            .expect("Missing CHROME_EXE_PATH='', add executable path to .env file.");

        // Set chrome profile name, fallbacks to default "profile1"
        let profile_name = params
            .as_ref()
            .and_then(|p| p.get("profile_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("profile1")
            .to_lowercase();

        // Sanitize profile_name, replaces spaces with underscores and allows a-z0-9, fallbacks to default "profile1"
        let mut sanitized_profile_name = String::with_capacity(profile_name.len());
        for c in profile_name.chars() {
            if c.is_alphanumeric() {
                sanitized_profile_name.push(c);
            } else if c == ' ' {
                sanitized_profile_name.push('_');
            }
        }
        let profile_name = if sanitized_profile_name.is_empty() {
            "profile1".to_string()
        } else {
            sanitized_profile_name
        };

        // Check if profile already exists in CDP_BROWSERS shared state for new target only
        if let Some(existing_profile_info) = CDP_BROWSERS.get_profile(&profile_name).await {
            // Reuse existing initial_cdp struct
            let initial_cdp = existing_profile_info.initial_cdp.clone();

            // Create new target within existing browser context
            let new_target_response = initial_cdp.send(
                "Target.createTarget",
                Some(json!({"url": "", "browserContextId": existing_profile_info.browser_context_id})),
            ).await?;
            let new_target_id = new_target_response
                .get_result()
                .get("targetId")
                .expect("No new targetId found")
                .as_str()
                .unwrap()
                .to_string();

            // Switch connection to new target
            let new_ws_url = format!(
                "ws://{}/devtools/page/{}",
                initial_cdp.cdp_host, new_target_id
            );
            let connected_cdp = initial_cdp.connect_to_ws(new_target_id, new_ws_url).await?;

            // Enable basic features
            if connected_cdp
                .get_params::<bool>("enable_basic_features")
                .unwrap_or(true)
            {
                connected_cdp.enable_basic_features().await?;
            }

            return Ok(connected_cdp);
        }

        // New initial_cdp (new chrome.exe instance):

        // Ensure port is unique across all profiles
        let debugger_port = match params
            .as_ref()
            .and_then(|p| p.get("args"))
            .and_then(|args| args.as_array())
            .and_then(|args| {
                args.iter().find_map(|arg| {
                    if let Some(arg_str) = arg.as_str() {
                        if arg_str.starts_with("--remote-debugging-port=") {
                            return arg_str.split('=').nth(1)?.parse::<u16>().ok();
                        }
                    }
                    None
                })
            }) {
            Some(port) => {
                if let Some(existing_profile_name) =
                    CDP_BROWSERS.get_profile_name_by_port(port).await
                {
                    warn!(
                        "Specified port --remote-debugging-port={} is already in use by profile '{}'. Switching to another port.",
                        port, existing_profile_name
                    );
                    find_free_port().await?
                } else {
                    port
                }
            }
            None => find_free_port().await?,
        };

        // Host, paths, ping interval..
        let cdp_host = format!("127.0.0.1:{}", debugger_port);
        let keep_alive_interval = Duration::from_secs(30);
        let exe_path = env::current_exe()?;
        let exe_dir = exe_path.parent().unwrap();
        let profiles_dir = exe_dir.join("tmp/browser");
        let user_data_dir = params
            .as_ref()
            .and_then(|p| p.get("args"))
            .and_then(|args| args.as_array())
            .and_then(|args| {
                args.iter().find_map(|arg| {
                    if let Some(arg_str) = arg.as_str() {
                        if arg_str.starts_with("--user-data-dir=") {
                            return Some(Path::new(arg_str.split('=').nth(1)?).to_path_buf());
                        }
                    }
                    None
                })
            })
            .unwrap_or(profiles_dir.join(&profile_name));
        // TODO:
        // - cleaning user profile as params, defaults to false or true or just manually by user ?
        // - this placement would delete profile before creating new, so move to handle_target_closed() for after cleanup
        // BE CAREFUL!
        //if let Err(e) = std::fs::remove_dir_all(/*&user_data_dir*/) {
        //    error!("Failed to remove profile directory: {}", e);
        //}

        // Start Chrome
        start_chrome(
            &user_data_dir,
            &chrome_exe_path,
            debugger_port,
            params.clone().unwrap_or_default(),
        )
        .await?;

        // Initialize event_handlers
        let event_handlers = Arc::new(Mutex::new(HashMap::new()));

        // Create WebSocketServer
        let ws_server_port = find_free_port().await?;
        let ws_server = WebSocketServer::new(format!("127.0.0.1:{}", ws_server_port)).await;

        // Create CDP instance
        let initial_cdp = Arc::new(CDP {
            cdp_host: cdp_host.clone(),
            keep_alive_interval,
            event_handlers,
            ws_server,
            connection: Arc::new(Connection {
                id: String::new(),
                request_id: Arc::new(AtomicU64::new(1)),
                ws_transmit: mpsc::unbounded_channel().0,
                ws_receive: Arc::new(Mutex::new(mpsc::unbounded_channel().1)),
                requests: Arc::new(Mutex::new(HashMap::new())),
            }),
            params: params.clone().unwrap_or_default(),
            profile_name: profile_name.clone(),
        });

        // Connect to browser debugger
        let url = format!("http://{}/json/version", cdp_host);
        if DEBUG {
            debug!("GET: {}", url);
        }
        let response = reqwest::get(url).await?.json::<Value>().await?;
        let version_target_id;
        let initial_cdp = if let Some(r) = response.as_object() {
            if let Some(ws_url) = r.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
                // Parse version_target_id from ws_url
                version_target_id = ws_url
                    .split('/')
                    .last()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Failed to parse version_target_id from ws_url")
                    })?
                    .to_string();

                initial_cdp
                    .connect_to_ws(version_target_id.to_string(), ws_url.to_string())
                    .await?
            } else {
                return Err(anyhow::anyhow!(
                    "Failed to extract 'webSocketDebuggerUrl' from response."
                ));
            }
        } else {
            return Err(anyhow::anyhow!("Response isnt object."));
        };

        // Check protocol data existence of CdpBrowsers protocol
        if CDP_BROWSERS.protocol.get().is_none() {
            let url = format!("http://{}/json/protocol", cdp_host);
            if DEBUG {
                debug!("GET: {}", url);
            }
            let response = reqwest::get(url).await?;
            let new_protocol: CdpProtocol = response.json().await?;
            CDP_BROWSERS
                .protocol
                .set(new_protocol.into())
                .unwrap_or_else(|_| {
                    //debug!("Protocol data was already set by another thread");
                });
        }

        // Create new browser context ("incognito")
        let created_browser_context_id = initial_cdp
            .send("Target.createBrowserContext", None)
            .await?
            .get_result()
            .get("browserContextId")
            .expect("No new browserContextId found")
            .as_str()
            .unwrap()
            .to_string();
        // Create target within new browser context
        let new_target_id = initial_cdp
            .send(
                "Target.createTarget",
                Some(json!({"url": "", "browserContextId": created_browser_context_id})),
            )
            .await?
            .get_result()
            .get("targetId")
            .expect("No new targetId found")
            .as_str()
            .unwrap()
            .to_string();

        // Add to CDP_BROWSERS shared state
        CDP_BROWSERS
            .add_profile(
                profile_name.clone(),
                version_target_id.clone(),
                debugger_port,
                created_browser_context_id.clone(),
                Arc::clone(&initial_cdp),
            )
            .await;

        // Switch connection to new target
        let ws_url = format!("ws://{}/devtools/page/{}", cdp_host, new_target_id);
        let connected_cdp = initial_cdp
            .connect_to_ws(new_target_id.clone(), ws_url)
            .await?;

        // Enable basic features
        if connected_cdp
            .get_params::<bool>("enable_basic_features")
            .unwrap_or(true)
        {
            connected_cdp.enable_basic_features().await?;
        }

        Ok(connected_cdp)
    }

    async fn setup_ctrlc_handler() {
        static INITIALIZED: OnceCell<()> = OnceCell::new();

        INITIALIZED.get_or_init(|| {
            tokio::spawn(async {
                signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");

                warn!("Ctrl+C detected. Shutting down. Press again if headless..");

                // collect profiles from lock
                let profile_infos: Vec<_> = {
                    let profiles = CDP_BROWSERS.profiles.lock().await;
                    profiles.values().cloned().collect()
                };

                // Iterate over all profiles and close their browsers
                let handles: Vec<_> = profile_infos
                    .into_iter()
                    .map(|info| {
                        let initial_cdp = info.initial_cdp.clone();
                        tokio::spawn(async move {
                            if let Err(e) = initial_cdp.send("Browser.close", None).await {
                                error!(
                                    "Failed to close browser for profile '{}': {:?}",
                                    info.debugger_id, e
                                );
                            }
                        })
                    })
                    .collect();

                // Wait for browsers to close
                for handle in handles {
                    let _ = handle.await;
                }

                if DEBUG {
                    debug!("All browsers closed. Exiting program.");
                }

                // Exit program
                std::process::exit(0);
            });
        });
    }

    // For reading params
    pub fn get_params<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.params
            .get(key)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    // Connecting or updating debugger connection
    async fn connect_to_ws(
        self: Arc<Self>,
        target_id: String,
        ws_url: String,
    ) -> Result<Arc<Self>> {
        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (write_tx, mut write_rx): (
            mpsc::UnboundedSender<Message>,
            mpsc::UnboundedReceiver<Message>,
        ) = mpsc::unbounded_channel();

        let (_read_tx, read_rx): (
            mpsc::UnboundedSender<Result<Message, tokio_tungstenite::tungstenite::Error>>,
            mpsc::UnboundedReceiver<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        ) = mpsc::unbounded_channel();

        let connection = Connection {
            id: target_id.clone(),
            request_id: Arc::new(AtomicU64::new(1)),
            ws_transmit: write_tx.clone(),
            ws_receive: Arc::new(Mutex::new(read_rx)),
            requests: Arc::new(Mutex::new(HashMap::new())),
        };

        let updated_cdp = Arc::new(CDP {
            connection: Arc::new(connection),
            ..(*self).clone()
        });

        tokio::spawn({
            async move {
                while let Some(msg) = write_rx.recv().await {
                    if ws_write.send(msg).await.is_err() {
                        debug!("Failed to send message, closing WebSocket.");
                        break;
                    }
                }
            }
        });

        tokio::spawn({
            let updated_cdp_clone = Arc::clone(&updated_cdp);
            async move {
                while let Some(msg) = ws_read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if DEBUG {
                                //debug!("CDP: {:?}", text.clone());
                            }
                            let resp = serde_json::from_str::<Value>(&text).unwrap();
                            let cdp_clone_inner = Arc::clone(&updated_cdp_clone);
                            tokio::spawn(async move {
                                if let Some(method) = resp.get("method").and_then(|m| m.as_str()) {
                                    cdp_clone_inner
                                        .handle_event(
                                            method,
                                            &resp,
                                            cdp_clone_inner.profile_name.clone(),
                                        )
                                        .await;
                                } else if let Some(req_id) =
                                    resp.get("id").and_then(|id| id.as_u64())
                                {
                                    let tx = {
                                        let mut pending_requests =
                                            cdp_clone_inner.connection.requests.lock().await;
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
                        Ok(Message::Pong(_)) => {}
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
            }
        });

        tokio::spawn({
            let updated_cdp_clone = Arc::clone(&updated_cdp);
            async move {
                loop {
                    tokio::time::sleep(updated_cdp_clone.keep_alive_interval).await;
                    if write_tx.send(Message::Ping(vec![])).is_err() {
                        debug!("Failed to send ping, WebSocket unavailable. Ping stopped.");
                        break;
                    }
                }
            }
        });

        Ok(updated_cdp)
    }

    pub async fn send(&self, method: &str, params: Option<Value>) -> Result<CdpResponse> {
        let reqid = self.connection.request_id.fetch_add(1, Ordering::SeqCst);
        if DEBUG {
            debug!("CDP -<: #{} @ {}", reqid, method);
        };

        let message = serde_json::to_value(SendPayload {
            id: reqid,
            method: method.to_string(),
            params,
        })?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending_requests = self.connection.requests.lock().await;
            pending_requests.insert(reqid, tx);
        }

        if self
            .connection
            .ws_transmit
            .send(Message::Text(message.to_string()))
            .is_err()
        {
            error!("WebSocket write channel is not available");
            return Err(anyhow::anyhow!("WebSocket write channel is not available"));
        }

        if DEBUG {
            debug!("CDP ~~: #{} @ {}", reqid, method);
        }

        let resp = rx.await??;
        if DEBUG {
            debug!("CDP >-: #{} @ {}: {}", reqid, method, resp);
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
                    error!("Unexpected:");
                }
            }

            Err(anyhow::anyhow!(
                "{}",
                json!({"error": resp["error"], "@": method, "protocol": "CDP", "#": reqid})
            ))
        }
    }

    async fn enable_basic_features(&self) -> Result<()> {
        self.send("DOM.enable", Some(json!({"includeWhitespace": "All"})))
            .await?;
        self.send("Page.enable", None).await?;
        self.send("Page.setBypassCSP", Some(json!({"enabled": true})))
            .await?;

        self.inject_ws_handler_script().await?;
        Ok(())
    }

    async fn inject_ws_handler_script(&self) -> Result<()> {
        let ws_server_addr = self.ws_server.addr();

        let js = format!(
            r#"
            (function() {{
                const rustWs = new WebSocket('ws://{}');

                window.sendToRustWs = function(message) {{
                    rustWs.send(JSON.stringify(message));
                }};

                window.setupRustWsHandlers = function(handlers) {{
                    rustWs.onopen = handlers.onOpen || function(event) {{}};
                    rustWs.onmessage = handlers.onMessage || function(event) {{}};
                    rustWs.onerror = handlers.onError || function(event) {{}};
                    rustWs.onclose = handlers.onClose || function(event) {{}};
                }};

                rustWs.addEventListener('message', function(event) {{
                    if (typeof window.rustWsHandlers !== 'undefined' && typeof window.rustWsHandlers.onMessage === 'function') {{
                        window.rustWsHandlers.onMessage(event);
                    }}
                }});
            }})();
            "#,
            ws_server_addr
        );

        self.send(
            "Page.addScriptToEvaluateOnNewDocument",
            Some(json!({
                "source": js,
            })),
        )
        .await?;
        Ok(())
    }

    async fn handle_invalid_params_error(&self, method: &str) {
        let protocol = CDP_BROWSERS
            .protocol
            .get()
            .expect("Protocol should be initialized");

        if let Some(domain) = protocol
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

    async fn handle_method_not_found_error(&self, method: &str) {
        let protocol = CDP_BROWSERS
            .protocol
            .get()
            .expect("Protocol should be initialized");

        let parts: Vec<&str> = method.split('.').collect();

        if parts.len() == 2 {
            let (domain_name, command_name) = (parts[0], parts[1]);

            let domain_matches: Vec<&Domain> = protocol
                .domains
                .iter()
                .filter(|domain| domain.domain.starts_with(domain_name))
                .collect();

            if domain_matches.is_empty() {
                error!(
                    "Domain '{}' wasn't found. Available domains: {:?}",
                    domain_name,
                    protocol
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
            let domain_matches: Vec<&Domain> = protocol
                .domains
                .iter()
                .filter(|domain| domain.domain.starts_with(domain_name))
                .collect();

            if domain_matches.is_empty() {
                error!(
                    "No domain found starting with '{}'. Available domains: {:?}",
                    domain_name,
                    protocol
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

    async fn update_node_ids(&self) -> Result<()> {
        // refresh nodeid's with getDocument
        self.send(
            "DOM.getDocument",
            Some(json!({
                "depth": -1,
                "pierce": true
            })),
        )
        .await?;
        Ok(())
    }

    async fn handle_event(&self, method: &str, resp: &Value, profile_name: String) {
        // Handle internal events first
        if self
            .get_params::<bool>("enable_basic_features")
            .unwrap_or(true)
        {
            if method == "DOM.documentUpdated" {
                if let Err(e) = self.update_node_ids().await {
                    error!("Failed to update root node ID: {:?}", e);
                }
            }
            if method == "Page.frameStoppedLoading" {
                if let Err(e) = self.update_node_ids().await {
                    error!("Failed to update root node ID: {:?}", e);
                }
            }
        }

        // Handle target closure events
        if method == "Inspector.detached" {
            if let Some(params) = resp.get("params") {
                if let Some(reason) = params.get("reason") {
                    if reason == "target_closed" {
                        // Here we can handle the case when a target is closed
                        if let Err(e) = self.handle_target_closed(profile_name).await {
                            error!("Failed to handle target closed: {:?}", e);
                        }
                    }
                }
            }
        }

        // Invoke user handlers
        let handlers = self.event_handlers.lock().await;
        if let Some(handler) = handlers.get(method) {
            (handler.handler)(resp);
        }
    }

    async fn handle_target_closed(&self, profile_name: String) -> Result<()> {
        // Fetch BrowserInfo to access initial_cdp
        if let Some(profile_info) = CDP_BROWSERS.get_profile(&profile_name).await {
            let initial_cdp = profile_info.initial_cdp.clone();

            // Use initial_cdp to query the list of current targets
            let targets_resp = initial_cdp
                .send("Target.getTargets", None)
                .await?
                .get_result();
            let target_infos = targets_resp
                .get("targetInfos")
                .expect("No targetInfos found");

            // If there are no more targets, trigger Browser.close
            if target_infos.as_array().map_or(0, |arr| arr.len()) == 0 {
                if DEBUG {
                    debug!(
                        "No more targets left in profile '{}', closing browser.",
                        profile_name
                    );
                }

                if let Err(e) = initial_cdp.send("Browser.close", None).await {
                    error!(
                        "Failed to close browser for profile '{}': {:?}",
                        profile_name, e
                    );
                }

                let mut profiles = CDP_BROWSERS.profiles.lock().await;
                profiles.remove(&profile_name);
                if DEBUG {
                    debug!("Removed profile '{}' from CDP_BROWSERS.", profile_name);
                }

                // Check if there are any CDP_BROWSERS left
                if profiles.is_empty() {
                    if DEBUG {
                        debug!("No CDP_BROWSERS left. Exiting program.");
                    }
                    // Exit program
                    std::process::exit(0);
                }
            }
        } else {
            warn!("Profile '{}' not found in CDP_BROWSERS.", profile_name);
        }

        Ok(())
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

    #[allow(dead_code)]
    pub async fn js_handle<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut cb = self.ws_server.callback.lock().await;
        *cb = Some(CallbackWrapper(Arc::new(callback)));
    }

    pub async fn send_to_js(&self, message: Value) -> Result<(), String> {
        self.ws_server.send(message).await
    }

    pub fn target_id(&self) -> String {
        self.connection.id.clone()
    }
}

async fn start_chrome(
    user_data_dir: &Path,
    chrome_exe_path: &str,
    debugger_port: u16,
    params: Value,
) -> Result<tokio::process::Child> {
    let mut chrome_args = vec![
        format!("--user-data-dir={}", user_data_dir.to_str().unwrap()),
        format!("--remote-debugging-port={}", debugger_port),
        format!("--remote-allow-origins=http://127.0.0.1:{}", debugger_port),
        "--no-default-browser-check".to_string(),
        "--disable-search-engine-choice-screen".to_string(),
        "--no-first-run".to_string(),
        "--no-startup-window".to_string(), // hide initial window
        "--incognito".to_string(),         // probably unnecessary, but kept for initial window mode
    ];

    let replace_or_push_arg = |chrome_args: &mut Vec<String>, user_arg: &str| {
        let prefix = user_arg.split('=').next().unwrap();
        if let Some(existing_arg) = chrome_args.iter_mut().find(|arg| arg.starts_with(prefix)) {
            *existing_arg = user_arg.to_string();
        } else {
            chrome_args.push(user_arg.to_string());
        }
    };

    if let Some(user_args) = params.get("args").and_then(|v| v.as_array()) {
        for arg in user_args {
            if let Some(arg_str) = arg.as_str() {
                replace_or_push_arg(&mut chrome_args, arg_str);
            }
        }
    }

    let enable_basic_features = params
        .get("enable_basic_features")
        .map(|v| v.as_bool().unwrap_or_else(|| v.as_str() == Some("true")))
        .unwrap_or(true);

    if enable_basic_features {
        let enable_basic_features_args = vec![
            "--disable-site-isolation-for-policy".to_string(),
            "--unsafely-disable-devtools-self-xss-warnings".to_string(),
            "--disable-infobars".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--disable-breakpad".to_string(),
            "--disable-sync".to_string(),
            "--disable-extensions".to_string(),
            "--v=1".to_string(),
            "--disable-default-apps".to_string(),
            "--disable-plugins-discovery".to_string(),
            "--disable-features=Translate".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
            "--disable-background-timer-throttling".to_string(),
        ];
        for arg in enable_basic_features_args {
            replace_or_push_arg(&mut chrome_args, &arg);
        }
    }

    if CHROME_STDERR {
        chrome_args.push("--enable-logging=stderr".to_string());
    }

    let child = Command::new(chrome_exe_path)
        .args(&chrome_args)
        .spawn()
        .context("Failed to start Chrome")?;

    Ok(child)
}

async fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
