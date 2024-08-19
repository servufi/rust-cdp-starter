use crate::cdp::CDP;
use anyhow::Result;
use async_trait::async_trait;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
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
        let mut rng = StdRng::from_entropy();

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
                sleep(Duration::from_millis(rng.gen_range(700..1000))).await;
            } else {
                first_run = false;
            }

            // Search scroll element by XPath
            if let Some(node_id_value) = self.find_node_id_by_xpath(scroll_to_xpath).await? {
                // initial scroll
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

                // Get box model for scroll start area (scroll_to_xpath element)
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
                        let scroll_x = rng.gen_range(
                            content[0].as_f64().unwrap_or(0.0)..content[4].as_f64().unwrap_or(0.0),
                        );
                        let scroll_y = rng.gen_range(
                            content[1].as_f64().unwrap_or(0.0)..content[5].as_f64().unwrap_or(0.0),
                        );

                        // infinite scroll jerking with mouseWheel event
                        if self
                            .send(
                                "Input.dispatchMouseEvent",
                                Some(json!({
                                    "type": "mouseWheel",
                                    "x": scroll_x,
                                    "y": scroll_y,
                                    "deltaX": 0,
                                    "deltaY": -60, // "scrollwheel tick" of 60px
                                    "pointerType": "mouse"
                                })),
                            )
                            .await
                            .is_err()
                        {
                            continue;
                        }

                        // Scroll to the target element
                        for _ in 0..rng.gen_range(5..15) {
                            if self
                                .send(
                                    "Input.dispatchMouseEvent",
                                    Some(json!({
                                        "type": "mouseWheel",
                                        "x": scroll_x,
                                        "y": scroll_y,
                                        "deltaX": 0,
                                        "deltaY": 60, // "scrollwheel tick" of 60px
                                        "pointerType": "mouse"
                                    })),
                                )
                                .await
                                .is_err()
                            {
                                continue;
                            }

                            // Check if 'until' element is reached
                            if let Some(_) = self.find_node_id_by_xpath(until_xpath_match).await? {
                                // final scroll
                                let _ = self
                                    .send(
                                        "DOM.scrollIntoViewIfNeeded",
                                        Some(json!({ "nodeId": node_id_value })),
                                    )
                                    .await;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
}
