# hornet [![Travis CI Build Status](https://travis-ci.org/performancecopilot/hornet.svg?branch=master)](https://travis-ci.org/performancecopilot/hornet) [![AppVeyor Build Status](https://ci.appveyor.com/api/projects/status/ccvbo3chne8046vn/branch/master?svg=true)](https://ci.appveyor.com/project/saurvs/hornet-2qtki/branch/master)

This is a work-in-progress PCP Memory Mapped Value (MMV) instrumentation API in Rust.

Currently, only singleton metrics are supported. There is a simple example of it's usage at `examples/physical.rs`. To run it, do

```
cargo run --example physical
```

If you have a valid PCP installation, the MMV file will be found at `$PCP_TMP_DIR/mmv/`. Otherwise, it'll be at `/tmp/mmv/`.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
