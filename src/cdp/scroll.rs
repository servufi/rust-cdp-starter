use crate::cdp::CDP;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tokio::time::{sleep, Duration};

#[async_trait]
#[allow(dead_code)]
pub trait Scroll {
    async fn wait_to_scroll_until_xpath(
        &self,
        scroll_to_xpath: &str,
        until_xpath_match: &str,
        timeout_secs: u64,
    ) -> Result<()>;
}

#[async_trait]
impl Scroll for CDP {
    async fn wait_to_scroll_until_xpath(
        &self,
        scroll_to_xpath: &str,
        until_xpath_match: &str,
        timeout_secs: u64,
    ) -> Result<()> {
        let start_time = tokio::time::Instant::now();
        let mut first_run = true;

        loop {
            if timeout_secs > 0
                && tokio::time::Instant::now().duration_since(start_time)
                    > Duration::from_secs(timeout_secs)
            {
                return Err(anyhow::anyhow!(
                    "Timeout reached while trying to scroll to element."
                ));
            }

            if !first_run {
                // Wait before retrying
                sleep(Duration::from_millis(1000)).await;
            } else {
                first_run = false;
            }

            // Search for scroll element by XPath
            if let Some(node_id_value) = self.find_node_id_by_xpath(scroll_to_xpath).await? {
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

                // Check if 'until' element is reached
                if let Some(_) = self.find_node_id_by_xpath(until_xpath_match).await? {
                    return Ok(());
                }
            }
        }
    }
}
