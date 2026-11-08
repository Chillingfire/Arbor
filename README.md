# Arbor
Multi-LLM coding harness for minimzing token usage and wasted compute

## Install

Download the archive for your platform from the
[latest release](https://github.com/Chillingfire/Arbor/releases/latest), check it
against `SHA256SUMS`, and put `arbor` on your `PATH`. Builds are published for
Windows x64, macOS arm64 and x64, and Linux x64 and arm64 (static musl).

Or build it with Cargo:

```sh
cargo install --git https://github.com/Chillingfire/Arbor --locked arbor
```

TODO: decide how `cargo install` should work. The names `arbor` and `arbor-core` are taken on
crates.io, so publishing would need different package names. Revisit once the repo is public.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Arbor by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
