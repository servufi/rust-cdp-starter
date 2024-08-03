use crate::cdp::CDP;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use tokio::time::{sleep, Duration};

#[async_trait]
pub trait Write {
    async fn wait_to_write_xpath(&self, xpath: &str, text: &str, timeout_secs: u64) -> Result<()>;
}

#[async_trait]
impl Write for CDP {
    async fn wait_to_write_xpath(&self, xpath: &str, text: &str, timeout_secs: u64) -> Result<()> {
        let start_time = tokio::time::Instant::now();
        let mut previous_search_id: Option<String> = None;

        loop {
            if timeout_secs > 0
                && tokio::time::Instant::now().duration_since(start_time)
                    > Duration::from_secs(timeout_secs)
            {
                return Err(anyhow::anyhow!(
                    "Timeout reached while waiting to write to element."
                ));
            }

            // Wait before retrying
            sleep(Duration::from_millis(1000)).await;

            // Cancel any previous search results
            if let Some(search_id) = &previous_search_id {
                let _ = self
                    .send(
                        "DOM.discardSearchResults",
                        Some(json!({ "searchId": search_id })),
                    )
                    .await;
            }

            // Perform search for element by XPath
            let search_result = match self
                .send(
                    "DOM.performSearch",
                    Some(json!({
                        "query": xpath.to_string(),
                        "includeUserAgentShadowDOM": true,
                    })),
                )
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    continue;
                }
            }
            .get_result();

            let search_id = search_result["searchId"]
                .as_str()
                .context("Failed to get searchId")?;

            previous_search_id = Some(search_id.to_string());

            let results_count = search_result["resultCount"]
                .as_u64()
                .context("Failed to get resultCount")?;

            if results_count > 0 {
                // Get first search result
                let results = match self
                    .send(
                        "DOM.getSearchResults",
                        Some(json!({
                            "searchId": search_id.to_string(),
                            "fromIndex": 0,
                            "toIndex": 1,
                        })),
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        continue;
                    }
                }
                .get_result();

                if let Some(node_id_value) = results
                    .get("nodeIds")
                    .and_then(|n| n.get(0))
                    .and_then(|v| v.as_i64())
                {
                    if self
                        .send(
                            "DOM.scrollIntoViewIfNeeded",
                            Some(json!({ "nodeId": node_id_value })),
                        )
                        .await
                        .is_err()
                    {
                        continue;
                    }

                    // Check if element is disabled or aria-disabled
                    let is_disabled = match self
                        .send(
                            "DOM.getAttributes",
                            Some(json!({ "nodeId": node_id_value })),
                        )
                        .await
                    {
                        Ok(attributes_result) => {
                            let result = attributes_result.get_result();
                            let attributes = match result["attributes"].as_array() {
                                Some(attrs) => attrs,
                                None => {
                                    continue;
                                }
                            };

                            attributes.chunks(2).any(|chunk| {
                                chunk[0].as_str() == Some("disabled")
                                    || (chunk[0].as_str() == Some("aria-disabled")
                                        && chunk[1].as_str() == Some("true"))
                            })
                        }
                        Err(_) => {
                            continue;
                        }
                    };
                    if is_disabled {
                        continue;
                    }

                    // Focus on input field
                    if self
                        .send("DOM.focus", Some(json!({ "nodeId": node_id_value })))
                        .await
                        .is_err()
                    {
                        continue;
                    }

                    // Insert text
                    if self
                        .send(
                            "Input.insertText",
                            Some(json!({
                                "text": text
                            })),
                        )
                        .await
                        .is_err()
                    {
                        continue;
                    }

                    // Cancel search
                    if self
                        .send(
                            "DOM.discardSearchResults",
                            Some(json!({ "searchId": search_id.to_string() })),
                        )
                        .await
                        .is_err()
                    {
                        continue;
                    }

                    return Ok(());
                }
            }
        }
    }
}
