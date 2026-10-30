# pi-rust-extensions

Extensions for [pi-rust](https://github.com/yannelli/pi-rust), written in Rust.

All extensions build into one binary, `pi-rust-ext`. Each release attaches the binary for each supported target, a signed `index.json`, and its signature. pi-rust checks the Ed25519 signature of the index and the SHA-256 of each download before it installs an extension.

## Install an extension

```sh
pi install marketplace:<name>
```

You can also open `/extensions` in the pi TUI to browse, install, and remove extensions.

## Layout

| Path | Contents |
| --- | --- |
| `crates/pi-ext-sdk` | Rust SDK for the pi-rust native extension protocol |
| `crates/pi-rust-ext` | The multi-call binary |
| `extensions/<name>` | One crate for each extension |
| `marketplace.json` | Catalog data for the release index |
