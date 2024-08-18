# Chrome Automation starter in Rust

A flexible chrome automation starter using Rust, leveraging Chrome DevTools Protocol (CDP) to control, inspect, and manipulate web pages. It features WebSocket communication between Rust <-> Browser and Rust <-> JavaScript.

## Features

- Automate chrome tasks using [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- WebSocket communication between Rust and JavaScript. Utilizes `Page.addScriptToEvaluateOnNewDocument` instead of `Runtime` in try to avoid automation detection. This means Rust <-> JS connection will be established for every iframe before hydrated content as well.
- Supports async operations for efficient task handling
- Easy to write CDP and JS event handlers (?)

## Getting Started

### Prerequisites

- Rust and Cargo installed
- Google Chrome installed
- Clone this repository:

```sh
git clone https://github.com/servufi/rust-cdp-starter.git
```

### Usage

1. Set up your `.env` file within same directory with `Cargo.toml`:

   `CHROME_EXE_PATH='C:\Program Files\Google\Chrome\Application\chrome.exe'`

   (Note single quotes)

2. Start developing from `src/main.rs`

3. ```sh
   cargo run
   ```

### Options

Basic features: DOM.enable, Page.enable, Page.setBypassCSP , inject_ws_handler_script() and multiple launch arguments:

```rust
   let browser1_params = Some(json!({
        "enable_basic_features": false, // enable_basic_features, default: true
    }));
```

`cdp.rs:`

```rust
   static DEBUG: bool = false; // toggle to see more stuff
```

Browser profiles at `./target/debug/tmp/browser` , to automatically delete profiles consider activating these lines:

```rust
   // TODO:
   // - cleaning user profile as params, defaults to false or true or just manually by user ?
   // - this placement would delete profile before creating new, so move to handle_target_closed() for after cleanup
   // BE CAREFUL!
   //if let Err(e) = std::fs::remove_dir_all(/*&user_data_dir*/) {
   //    error!("Failed to remove profile directory: {}", e);
   //}
```

### Example

```rust
   // Start headed browser with default params
   let github = CDP::new(None).await?;

   // resize window
   github.send(
      "Browser.setWindowBounds",
      Some(json!({
         "windowId": github.send("Browser.getWindowForTarget", None).await?.as_i32("windowId")?,
         "bounds": json!({
               "top": 0,
               "left": 0,
               "width": 1024,
               "height": 420,
         })
      })),
   )
   .await?;

   // Navigate to github
   github
      .send("Page.navigate", Some(json!({"url": "https://github.com"})))
      .await?;
```

More @ [`src/main.examples.rs`](https://github.com/servufi/rust-cdp-starter/blob/main/src/main.examples.rs)

### Bugs

Quite sure some oddities may occur, happy to hear of them :)
