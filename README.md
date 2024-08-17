# Chrome Automation starter in Rust

A flexible chrome automation starter built using Rust, leveraging Chrome DevTools Protocol (CDP) to control, inspect, and manipulate web pages. It features WebSocket communication between Rust <-> Browser and Rust <-> JavaScript.

## Features

- Automate chrome tasks using [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- WebSocket communication between Rust and JavaScript
- Supports async operations for efficient task handling
- Utilizes `Page.addScriptToEvaluateOnNewDocument` instead of `Runtime` to help avoid automation detection. This also means Rust<->JS connection will be established for each iframe context
- Easy(?) to write CDP and JS event handlers

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
   let tab = Some(json!({
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
   // CAREFUL!
   if let Err(e) = std::fs::remove_dir_all(/*&user_data_dir*/) {
       error!("Failed to remove profile directory: {}", e);
   }
```

### Examples

@ `src/main.rs`

### Bugs

Quite sure some oddities occur, happy to hear :)
