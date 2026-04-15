# The Carrot Rust Extension API

This crate lets you write extensions for Carrot in Rust.

## Extension Manifest

You'll need an `extension.toml` file at the root of your extension directory, with the following structure:

```toml
id = "my-extension"
name = "My Extension"
description = "..."
version = "0.0.1"
schema_version = 1
authors = ["Your Name <you@example.com>"]
repository = "https://github.com/your/extension-repository"
```

## Cargo metadata

Carrot extensions are packaged as WebAssembly files. In your Cargo.toml, you'll
need to set your `crate-type` accordingly:

```toml
[dependencies]
carrot_extension_api = "0.6.0"

[lib]
crate-type = ["cdylib"]
```

## Implementing an Extension

To define your extension, create a type that implements the `Extension` trait, and register it.

```rust
use carrot_extension_api as carrot;

struct MyExtension {
    // ... state
}

impl carrot::Extension for MyExtension {
    // ...
}

carrot::register_extension!(MyExtension);
```

## Testing your extension

To run your extension in Carrot as you're developing it:

- Make sure you have [Rust installed](https://www.rust-lang.org/learn/get-started)
- Have the `wasm32-wasip2` target installed (`rustup target add wasm32-wasip2`)
- Open the extensions view using the `carrot: extensions` action in the command palette.
- Click the `Install Dev Extension` button in the top right
- Choose the path to your extension directory.

## Compatible Carrot versions

Extensions created using newer versions of the Carrot extension API won't be compatible with older versions of Carrot.

Here is the compatibility of the `carrot_extension_api` with versions of Carrot:

| Carrot version | `carrot_extension_api` version |
| -------------- | ------------------------------ |
| `0.1.x`        | `0.0.1`                        |
