use crate::cdp::CDP;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tokio::time::{sleep, Duration};

#[async_trait]
#[allow(dead_code)]
pub trait Write {
    async fn wait_to_write_xpath(&self, xpath: &str, text: &str, timeout_secs: u64) -> Result<()>;
}

#[async_trait]
impl Write for CDP {
    async fn wait_to_write_xpath(&self, xpath: &str, text: &str, timeout_secs: u64) -> Result<()> {
        let start_time = tokio::time::Instant::now();
        let mut first_run = true;

        loop {
            if timeout_secs > 0
                && tokio::time::Instant::now().duration_since(start_time)
                    > Duration::from_secs(timeout_secs)
            {
                return Err(anyhow::anyhow!(
                    "Timeout reached while waiting to write to element."
                ));
            }

            if !first_run {
                // Wait before retrying
                sleep(Duration::from_millis(1000)).await;
            } else {
                first_run = false;
            }

            // Find node by XPath
            let node_id_value = match self.find_node_id_by_xpath(xpath).await? {
                Some(id) => id,
                None => continue,
            };

            // Scroll element into view
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
                        None => continue,
                    };

                    attributes.chunks(2).any(|chunk| {
                        chunk[0].as_str() == Some("disabled")
                            || (chunk[0].as_str() == Some("aria-disabled")
                                && chunk[1].as_str() == Some("true"))
                    })
                }
                Err(_) => continue,
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

            return Ok(());
        }
    }
}
