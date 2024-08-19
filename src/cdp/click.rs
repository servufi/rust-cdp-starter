use crate::cdp::CDP;
use anyhow::Result;
use async_trait::async_trait;
use log::error;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use tokio::time::{sleep, Duration};

#[async_trait]
#[allow(dead_code)]
pub trait Click {
    async fn wait_to_click_xpath(
        &self,
        xpath: &str,
        click_count: u32,
        timeout_secs: u64,
    ) -> Result<()>;
}

#[async_trait]
impl Click for CDP {
    async fn wait_to_click_xpath(
        &self,
        xpath: &str,
        click_count: u32,
        timeout_secs: u64,
    ) -> Result<()> {
        // Thread-safe RNG
        let mut rng = StdRng::from_entropy();
        let start_time = tokio::time::Instant::now();
        let mut clicks_remaining = click_count;
        let mut first_run = true;

        loop {
            if timeout_secs > 0
                && tokio::time::Instant::now().duration_since(start_time)
                    > Duration::from_secs(timeout_secs)
            {
                return Err(anyhow::anyhow!(
                    "Timeout reached while waiting to click element."
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

            // Scroll element into view
            if self
                .send(
                    "DOM.scrollIntoViewIfNeeded",
                    Some(json!({ "nodeId": node_id_value })),
                )
                .await
                .is_err()
            {
                error!("Failed to scroll element into view.");
                continue;
            }

            // Get box model for calculating click coordinates
            let box_model = match self
                .send("DOM.getBoxModel", Some(json!({ "nodeId": node_id_value })))
                .await
            {
                Ok(result) => result,
                Err(_) => continue,
            }
            .get_result();

            if let Some(model) = box_model.get("model") {
                let content = model.get("content").and_then(|c| c.as_array());
                if let Some(content) = content {
                    if content.len() == 8 {
                        let x1 = content[0].as_f64().unwrap_or(0.0);
                        let y1 = content[1].as_f64().unwrap_or(0.0);
                        let x2 = content[4].as_f64().unwrap_or(0.0);
                        let y2 = content[5].as_f64().unwrap_or(0.0);

                        let width = (x2 - x1).abs();
                        let height = (y2 - y1).abs();

                        // Ensure element is within the viewport
                        if width > 0.0 && height > 0.0 {
                            // Randomize click location within elements bounding box
                            let click_x = rng.gen_range(x1..x2);
                            let click_y = rng.gen_range(y1..y2);

                            // Use node_id position directly for the click
                            if let Err(e) = self
                                .send(
                                    "Input.dispatchMouseEvent",
                                    Some(json!({
                                        "type": "mousePressed",
                                        "x": click_x,
                                        "y": click_y,
                                        "button": "left",
                                        "clickCount": 1,
                                    })),
                                )
                                .await
                            {
                                error!("Failed to dispatch mouse press event: {:?}", e);
                                continue;
                            }

                            sleep(Duration::from_millis(rng.gen_range(10..30))).await;

                            if let Err(e) = self
                                .send(
                                    "Input.dispatchMouseEvent",
                                    Some(json!({
                                        "type": "mouseReleased",
                                        "x": click_x,
                                        "y": click_y,
                                        "button": "left",
                                        "clickCount": 1,
                                    })),
                                )
                                .await
                            {
                                error!("Failed to dispatch mouse release event: {:?}", e);
                                continue;
                            }

                            clicks_remaining -= 1;
                            if clicks_remaining <= 0 {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
}
