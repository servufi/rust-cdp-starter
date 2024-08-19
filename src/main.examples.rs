mod cdp;
use anyhow::Result;
use cdp::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger
    env_logger::Builder::from_default_env()
        .filter(None, log::LevelFilter::Debug) // Global
        .filter_module("reqwest", log::LevelFilter::Info) // Set reqwest to info level
        .filter_module("tungstenite", log::LevelFilter::Info) // Set tungstenite to info level
        .format_timestamp_millis()
        .init();

    // Define headless browser params
    let headless1_params = Some(json!({
        "enable_basic_features": true, // enable_basic_features, default: true
        "profile_name": "profile0", // new profile name = new browser, default: "profile1" (sanitized to: a-z0-9 spaces replaced with _ and to lowercase)
        "args": [ // Optional chrome launch args
            "--headless=new", // start with new headless commands (--headless = --headless=old)
            "--remote-debugging-port=13236", // if profile name is running this arg will be ignored and running port will be used instead. default: find_free_port()
        ],
    }));
    // Start couple headless tabs within same browser
    let headless1_tab1 = CDP::new(headless1_params.clone()).await?;
    let _headless1_tab2 = CDP::new(headless1_params.clone()).await?;

    // Use of default headed browser params
    let browser1_tab1 = CDP::new(None).await?; // with defaults
    let browser1_tab2 = CDP::new(None).await?; // with defaults of browser1

    // Each new browser window requires new unique profile name, reusing same will create new tab in corresponding browser
    let browser2_params = Some(json!({
        "profile_name": "profile2", // new profile name = new browser, default: "profile1" (sanitized to: a-z0-9 spaces replaced with _ and to lowercase)
        "args": [ // Optional chrome launch args
            "--incognito", // incognito argument does nothing. (incognito is default for initial hidden window, next targets are in own browser contexts like incognito)
        ],
    }));
    // Start browser2 tab1
    let browser2_tab1 = CDP::new(browser2_params).await?;

    // Define ws_server end handler for incoming JS messages
    let browser2_tab1_clone = browser2_tab1.clone();
    browser2_tab1
        .js_handle(move |message| {
            // Parse message (assuming it is JSON)
            if let Ok(parsed_message) = serde_json::from_str::<serde_json::Value>(&message) {
                if let Some(context_id) = parsed_message.get("contextId").and_then(|v| v.as_str()) {
                    // Process messages by contextId
                    if context_id.contains("google") {
                        info!("Processing message from context: {}", context_id);
                        let response = json!({
                            "contextId": context_id,
                            "response": format!("Received '{}' , hello from 'tab' ws_server", message),
                        });

                        let browser2_tab1_clone_inner = browser2_tab1_clone.clone();
                        tokio::spawn(async move {
                            let _ = browser2_tab1_clone_inner.send_to_js(response).await;
                        });
                    }
                }
            }
        })
        .await;

    // Define target JS WS handlers for each new document such as iframes. (Each browser should have own ws_server between all tabs)
    // If intereested refer to 'src/cdp/cdp.rs' to find out JS part which connects to ws_server and provides functions here in example.
    // If not, just add context identifiers in this script and adjust server side cdp.js_handle accordingly.
    let target_js = r#"
        (async function() {
            // Wait for the Rust WebSocket setup to complete
            function sleep(ms) {
                return new Promise(resolve => setTimeout(resolve, ms));
            }
            for (let i = 0; i < 30; i++) {
                if (typeof setupRustWsHandlers === 'function') {
                    break;
                }
                await sleep(1000);
            }
            if (typeof setupRustWsHandlers !== 'function') {
                console.error('setupRustWsHandlers not found, exiting...');
                return;
            }

            // WS server event handlers
            const contextId = `${document.referrer} -> ${document.location.href}`;
            if (contextId !== 'about:blank') {
                setupRustWsHandlers({
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
                    if (typeof sendToRustWs === 'function') {
                        sendToRustWs({ contextId, data: message });
                    }
                    counter++;
                }, 5000);
            }
        })();
    "#;

    // Register to handle firing CDP events
    headless1_tab1
        .register_event_handler("Page.frameStoppedLoading", |message| {
            info!(
                "headless1_tab1: Page.frameStoppedLoading event: {:?}",
                message
            );
        })
        .await;

    // Remove CDP event handler
    //headless1_tab1.unregister_event_handler("Page.frameStoppedLoading")
    //    .await;

    // Send commands to different locations:
    //  Evaluate above target_js on each new document (including each iframe) in browser2_tab1
    browser2_tab1
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            Some(json!({
                "source": target_js
            })),
        )
        .await?;

    //  Navigate browser1_tab2
    browser1_tab2
        .send(
            "Page.navigate",
            Some(json!({"url": "https://duckduckgo.com"})),
        )
        .await?;

    //  Navigate browser2_tab1
    browser2_tab1
        .send("Page.navigate", Some(json!({"url": "https://google.com"})))
        .await?;

    //  Navigate headless1_tab1
    headless1_tab1
        .send(
            "Page.navigate",
            Some(json!({"url": "https://github.com/servufi/rust-cdp-starter"})),
        )
        .await?;

    //  Async task example which clicks "accept" Google GDPR in those regions
    let browser2_tab1_clone = browser2_tab1.clone();
    tokio::spawn(async move {
        let _ = browser2_tab1_clone
            .wait_to_click_xpath(
                // flimsy xpath!
                "/html/body/div[2]/div[2]/div[3]/span/div/div/div/div[3]/div[1]/button[1]", // target xpath surface
                1, // click 1 times in total whenever xpath matches
                5, // wait xpath match for total of 5 seconds, stops when done or timeout is reached
            )
            .await;
    });

    //  Should insert text into search field
    browser2_tab1
        .wait_to_write_xpath(
            "//textarea[@role='combobox']",                // target xpath
            "https://github.com/servufi/rust-cdp-starter", // text to write
            0, // waits for xpath match forever, stops when done
        )
        .await?;

    // "low detection"..
    browser1_tab1
        .send(
            "Page.navigate",
            Some(json!({"url": "https://www.browserscan.net/bot-detection"})),
        )
        .await?;
    let browser1_tab1_clone = browser1_tab1.clone();
    tokio::spawn(async move {
        let _ = browser1_tab1_clone
            .wait_to_click_xpath(
                "//div[contains(@class, 'fc-consent-root')]//button[contains(@class, 'fc-close') and contains(@class, 'fc-icon-button') and @aria-label='Close']", // target xpath surface
                1, // click 1 times in total whenever xpath matches
                10, // wait xpath match for total of 10 seconds, stops when done or timeout is reached
            )
            .await;
    });

    // Activate target ("activate / focus site")
    browser1_tab1
        .send(
            "Target.activateTarget",
            Some(json!({
                "targetId": browser1_tab1.target_id(),
            })),
        )
        .await?;

    // Set window bounds
    browser1_tab1
        .send(
            "Browser.setWindowBounds",
            Some(json!({
                "windowId": browser1_tab1.send("Browser.getWindowForTarget", None).await?.as_i32("windowId")?,
                "bounds": json!({
                    "top": 0,
                    "left": 0,
                    "width": 1024,
                    "height": 768,
                })
            })),
        )
        .await?;

    // Send something to all target_js
    let _ = browser2_tab1
        .send_to_js(json!({ "contextId": "maybeSomeidentifierEtc", "response": "Hi from rust"}))
        .await;

    /*
    // "wait_to_scroll_until_xpath" example. Using same xpath for "scroll_to_xpath" and "until_xpath_match" would scroll to it once.
    linkedin // fpr example you would have 'linkedin' -jobs initialized
        .wait_to_scroll_until_xpath(
            "//icon[contains(@class, 'li-footer')]", // target element to keep scrolling to ..
            "//button[contains(@class, 'show-more-button--visible')]
            |
            //div[contains(@class, 'see-more-jobs__viewed-all') and not(contains(@class, 'hidden'))]", // .. until either of these elements in xpath is found or ..
            30, // .. stop scrolling after 30 seconds. ( 0 would never timeout )
        )
        .await?;
    */

    // Keep running
    loop {
        sleep(Duration::from_secs(30)).await;
    }
}
