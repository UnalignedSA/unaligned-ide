# Cli

## Testing

You can test your changes to the `cli` crate by first building the editor's binary:

```
cargo build -p ide
```

And then building and running the `cli` crate with the following parameters:

```
 cargo run -p cli -- --app ./target/debug/unaligned-ide.exe
```
