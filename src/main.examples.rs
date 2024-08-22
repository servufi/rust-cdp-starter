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

    // Example: Define headless browser params
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

    // Example: Use default headed browser params
    let browser1_tab1 = CDP::new(None).await?; // with defaults
    let browser1_tab2 = CDP::new(None).await?; // with defaults of browser1

    // Example: Each additional new browser window requires own new unique profile name, reusing same will create new tab in existing browser window
    let browser2_params = Some(json!({
        "profile_name": "profile2", // new profile name = new browser, default: "profile1" (sanitized to: a-z0-9 spaces replaced with _ and to lowercase)
        "args": [ // Optional chrome launch args
            "--incognito", // incognito argument does nothing. (incognito is default for initial hidden window, next targets are in own browser contexts like incognito)
        ],
    }));
    // Start browser2 tab1
    let browser2_tab1 = CDP::new(browser2_params).await?;

    // Example: Define .js_handle for incoming JS messages of all tab contexts
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
                            let _ = browser2_tab1_clone_inner.broadcast_to_js(response).await;
                        });
                    }
                }
            }
        })
        .await;

    // Define target JS WS handlers for each new document such as iframes. Each tab has dedicated ws_server.
    // If intereested refer to 'src/cdp/cdp.rs' to find out JS part which connects to ws_server and provides functions here in example.
    // If not, just modify this script and adjust server side browser2_tab1.js_handle accordingly.
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

    // Example: Register to handle firing CDP events
    headless1_tab1
        .register_event_handler("Page.frameStoppedLoading", |message| {
            info!(
                "headless1_tab1: Page.frameStoppedLoading event: {:?}",
                message
            );
        })
        .await;

    // Example: Remove CDP event handler
    //headless1_tab1.unregister_event_handler("Page.frameStoppedLoading")
    //    .await;

    // Example: Evaluate above target_js on each new document (including each iframe) within browser2_tab1
    browser2_tab1
        .send(
            "Page.addScriptToEvaluateOnNewDocument",
            Some(json!({
                "source": target_js
            })),
        )
        .await?;

    // Example:  Navigate different tabs
    browser1_tab2
        .send(
            "Page.navigate",
            Some(json!({"url": "https://duckduckgo.com"})),
        )
        .await?;

    browser2_tab1
        .send("Page.navigate", Some(json!({"url": "https://google.com"})))
        .await?;

    headless1_tab1
        .send(
            "Page.navigate",
            Some(json!({"url": "https://github.com/servufi/rust-cdp-starter"})),
        )
        .await?;

    // Example: Async task which clicks "accept" Google GDPR in those regions
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

    // Example: insert text into search field
    browser2_tab1
        .wait_to_write_xpath(
            "//textarea[@role='combobox']",                // target xpath
            "https://github.com/servufi/rust-cdp-starter", // text to write
            0, // waits for xpath match forever, stops when done
        )
        .await?;

    // Example: "low detection"..
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

    // Example: Activate target ("activate / focus site")
    browser1_tab1
        .send(
            "Target.activateTarget",
            Some(json!({
                "targetId": browser1_tab1.target_id(),
            })),
        )
        .await?;

    // Example: Set window bounds
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

    // Example: broadcast something to all target_js in single tab
    let _ = browser2_tab1
        .broadcast_to_js(
            json!({ "contextId": "maybeSomeidentifierEtc", "response": "Hi from rust"}),
        )
        .await;

    // Example: Data extraction example using JS textContent:
    //  This JS textContent approach requires open WS and loaded content to get results, thus tied to firing CDP event, because .text_content_by_xpath() only broadcasts
    //   request for all JS contexts over open ws_server and then they send results back to .js_handle where as "executionContextId" identifier (from avoided Runtime domain)
    //   xpath could be maybe used instead, but it would still not distinct execution contexts within tab accurately.
    //
    //  Note: Other more robust option is .find_node_id_by_xpath() or find_node_ids_by_xpath(), then 'DOM.getOuterHTML' and lastly use 'scraper' -crate to parse HTML to extract
    //   text between tags safely without any JS.

    let linkedin = CDP::new(None).await?;

    let data_xpath =
        "//div[contains(@class, 'content-hub-entity-card-redesign')]//h2[contains(@class, 'mb-1')]";

    linkedin
        .js_handle(move |message| {
            // Parse the incoming message (assuming it is JSON)
            if let Ok(parsed_message) = serde_json::from_str::<serde_json::Value>(&message) {
                // Check if the message contains results for specific XPath
                if let Some(xpath) = parsed_message
                    .get("getTextContentsXpath")
                    .and_then(|x| x.as_str())
                {
                    if xpath == data_xpath {
                        // Check if the results field is array
                        if let Some(results) =
                            parsed_message.get("results").and_then(|r| r.as_array())
                        {
                            println!("Results for XPath '{}':", xpath);
                            for result in results {
                                if let Some(text_content) =
                                    result.get("textContent").and_then(|tc| tc.as_str())
                                {
                                    println!("Text Content: {}", text_content);
                                }
                            }
                        }
                    }
                }
            }
        })
        .await;

    let linkedin_clone = linkedin.clone();
    linkedin
        .register_event_handler("Page.frameStoppedLoading", move |message| {
            let linkedin_clone = linkedin_clone.clone();
            let message_clone = message.clone();
            tokio::spawn(async move {
                info!(
                    "linkedin: Page.frameStoppedLoading event: {:?}",
                    message_clone
                );
                // "Run once"
                linkedin_clone
                    .unregister_event_handler("Page.frameStoppedLoading")
                    .await;

                // try to dismiss prompts at least 2 times within 20s
                let linkedin_inner = linkedin_clone.clone();
                tokio::spawn(async move {
                    let _ = linkedin_inner
                        .wait_to_click_xpath("//button[contains(@aria-label, 'Dismiss')] | //button[contains(@data-control-name, 'ga-cookie.consent') and @action-type='DENY']", 2, 20)
                        .await;
                });

                // Broadcast text_content_by_xpath request for all target JS ( catch results in linkedin.js_handle() )
                linkedin_clone
                    .text_content_by_xpath(data_xpath)
                    .await;
            });
        })
        .await;

    linkedin
        .send(
            "Page.navigate",
            Some(json!({ "url": "https://www.linkedin.com/pulse/topics/home/"})),
        )
        .await?;

    // Example: "wait_to_scroll_until_xpath". Using same xpath for "scroll_to_xpath" and "until_xpath_match" would scroll to it once.
    linkedin.wait_to_click_xpath("//a[contains(@href, 'https://www.linkedin.com/jobs/search?trk=content-hub-home-page_guest_nav_menu_jobs')]", 1, 20).await?;
    linkedin
        .wait_to_scroll_until_xpath(
            "//icon[contains(@class, 'li-footer')]", // target element to keep scrolling to ..
            "//button[contains(@class, 'show-more-button--visible')]
            |
            //div[contains(@class, 'see-more-jobs__viewed-all') and not(contains(@class, 'hidden'))]", // .. until either of these elements in xpath is found or ..
            20, // .. stop scrolling after 20 seconds. ( 0 would never timeout )
        )
        .await?;

    // Keep running
    loop {
        sleep(Duration::from_secs(30)).await;
    }
}
