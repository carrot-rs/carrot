# Cli

## Testing

You can test your changes to the `cli` crate by first building the main carrot binary:

```
cargo build -p carrot-app
```

And then building and running the `cli` crate with the following parameters:

```
 cargo run -p cli -- --carrot ./target/debug/carrot.exe
```
