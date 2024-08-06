mod cdp;
use anyhow::Result;
use cdp::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger
    env_logger::Builder::from_default_env()
        .filter(None, log::LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    // Initialize CDP (first init requires couple boots to generate chrome profile)
    let cdp = CDP::new(true).await?;

    // Set window bounds
    let _ = cdp
        .send(
            "Browser.setWindowBounds",
            Some(json!({
                "windowId": cdp.send("Browser.getWindowForTarget", None).await?.get_i32("windowId")?,
                "bounds": json!({
                    "top": 0,
                    "left": 0,
                    "width": 1024,
                    "height": 768,
                })
            })),
        )
        .await;

    // Define ws_server handler for JS messages
    let cdp_clone = cdp.clone();
    cdp.js_handle(move |message| {
        info!("ws_server received message from JS: {}", message);
        // Send back response asynchronously
        let cdp_clone_inner = cdp_clone.clone();
        tokio::spawn(async move {
            let response = format!("Received '{}', hello from Rust", message);
            cdp_clone_inner.ws_server.send(&response).await;
        });
    })
    .await;

    // JS WS handlers example for each new document such as iframes.
    // If intereested refer to 'src/cdp/cdp.rs' to find out JS part which connects to ws_server and provides functions here in example.
    // If not, just add context identifiers in this script and adjust server side cdp.js_handle accordingly.
    let _ = cdp
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            Some(json!({
                "source": r#"
                (function() {
                    // WebSocket server event handlers
                    const contextId = document.location.href;
                    setupWsHandlers({
                        onOpen: function(event) {
                            console.log(`[${contextId}] Custom WebSocket open handler:`, event);
                        },
                        onMessage: function(event) {
                            console.log(`[${contextId}] Custom WebSocket message handler:`, event.data);
                        },
                        onError: function(event) {
                            console.error(`[${contextId}] Custom WebSocket error handler:`, event);
                        },
                        onClose: function(event) {
                            console.log(`[${contextId}] Custom WebSocket close handler:`, event);
                        }
                    });

                    // Send to WebSocket server
                    let counter = 0;
                    setInterval(() => {
                        const message = `Message ${counter} from JS context: ${contextId}`;
                        if (typeof sendToWs === 'function') {
                            sendToWs({ data: message });
                        }
                        console.log(`Sent: ${message}`);
                        counter++;
                    }, 5000);
                })();
            "#
            })),
        )
        .await;

    // Define CDP event handler example
    cdp.register_event_handler("Page.frameStoppedLoading", |message| {
        info!("Page.frameStoppedLoading event: {:?}", message);
    })
    .await;

    // Remove CDP event handler example
    //cdp.unregister_event_handler("Page.frameStoppedLoading")
    //    .await;

    // Navigate to website
    let _ = cdp
        .send(
            "Page.navigate",
            Some(json!({
                "url": "https://www.google.com"
            })),
        )
        .await;

    // Async task example which accepts GDPR in those regions
    let cdp_clone = cdp.clone();
    tokio::spawn(async move {
        let _ = cdp_clone
            .wait_to_click_xpath(
                // flimsy xpath!
                "/html/body/div[2]/div[2]/div[3]/span/div/div/div/div[3]/div[1]/button[1]",
                2,
                10,
            )
            .await;
    });

    // sleep and Duration imported in prelude, check 'src/cdp/mod.rs' what else is
    warn!("Sleeping for 4 seconds (to avoid site collisions with async task example)...");
    sleep(Duration::from_secs(4)).await;

    // Should insert text into search field
    cdp.wait_to_write_xpath(
        "//textarea[@role='combobox']",
        "https://github.com/servufi/rust-cdp-starter",
        0,
    )
    .await?;

    // Keep running
    loop {
        sleep(Duration::from_secs(30)).await;
    }
}
