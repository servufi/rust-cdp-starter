# Browser Automation starter in Rust

A flexible browser automation starter built using Rust, leveraging Chrome DevTools Protocol (CDP) to control, inspect, and manipulate web pages. It features WebSocket communication between Rust <-> Browser and Rust <-> JavaScript.

## Features

- Automate browser tasks using [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/)
- WebSocket communication between Rust and JavaScript contexts
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

2. Modify the `src/main.rs` file, or whatever, to define your automation tasks and event handlers.

3. ```sh
   cargo run
   ```

### Options

```rust
   let cdp = CDP::new(true).await?; // With features, quick start
   let cdp = CDP::new(false).await?; // Manual mode, only CDP
```

`cdp.rs:`

```rust
   static DEBUG: bool = false; // toggle to see more stuff
```

If needed add more time here when initializing Chrome profile first time on slower computers. To restart profile initialization sequence delete profile folder: `target/debug/tmp`

```rust
   if no_user_data {
      warn!("Chrome profile not found, stopping to generate one..");
       // Might take longer to generate on slower computers
      tokio::time::sleep(Duration::from_secs(7)).await;
      let _ = cdp.send("Browser.close", None).await;
      std::process::exit(0);
   }
```

### Examples

@ `main.rs`

### Bugs

Quite sure some oddities occur, happy to hear :)
