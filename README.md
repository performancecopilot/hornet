# hornet [![crates.io badge](https://img.shields.io/crates/v/hornet.svg)](https://crates.io/crates/hornet) [![docs.rs badge](https://docs.rs/hornet/badge.svg)](https://docs.rs/hornet/0.1.0/hornet/) [![Travis CI Build Status](https://travis-ci.org/performancecopilot/hornet.svg?branch=master)](https://travis-ci.org/performancecopilot/hornet) [![AppVeyor Build Status](https://ci.appveyor.com/api/projects/status/ccvbo3chne8046vn/branch/master?svg=true)](https://ci.appveyor.com/project/saurvs/hornet-2qtki/branch/master) [![codecov](https://codecov.io/gh/performancecopilot/hornet/branch/master/graph/badge.svg)](https://codecov.io/gh/performancecopilot/hornet)

`hornet` is a Performance Co-Pilot (PCP) Memory Mapped Values (MMV) instrumentation library written in Rust.

**Contents**

* [What is PCP MMV instrumentation?](#what-is-pcp-mmv-instrumentation)
* [Usage](#usage)
* [API](#api)
  * [Singleton Metric](#singleton-metric)
  * [Instance Metric](#instance-metric)
  * [Special Metrics](#special-metrics)
  * [Client](#client)
  * [Monitoring metrics](#monitoring-metrics)
* [License](#license)

## What is PCP MMV instrumentation?

[Performance Co-Pilot](http://pcp.io/) is a systems performance analysis framework with a distributed and scalable architecture. It supports a low overhead
method for instrumenting applications called Memory Mapped Values (MMV), in which instrumented processes share part of their virtual memory address space with another monitoring process through a common memory-mapped file. The shared address space contains various performance analysis metrics stored in a structured binary data format called MMV; it's formal spec can be found [here](http://pcp.io/man/man5/mmv.5.html). When processes wish to update their metrics, they simply write certain bytes to the memory mapped file, and the monitoring process reads it at appropriate times. No explicit inter-process communication, synchronization or systems calls are involved.

## Usage

* Add the ```hornet``` dependency to your ```Cargo.toml```
  ```toml
  [dependencies]
  hornet = "0.1.0"
  ```

* Include the ```hornet``` crate in your code and import the following modules
  ```rust
  extern crate hornet;

  use hornet::client::Client;
  use hornet::client::metric::*;
  ```

## API

There are essentially two kinds of metrics in `hornet`.

### Singleton Metric

A singleton metric is a metric associated with a primitive value type, a `Unit`, a `Semantics` type, and some metadata. A primitive value can be any one of `i64`, `u64`, `i32`, `u32`, `f64`, `f32`, or `String`, 

The primitive value type of a metric is determined implicitly at *compile-time* by the inital primitive value passed to the metric while creating it. The programmer also needn't worry about reading or writing data of the wrong primitive type from a metric, as the Rust compiler enforces type safety for a metric's primitive value during complilation.

Let's look at creating a simple `i64` metric

  ```rust
  let mut metric = Metric::new(
      "simple", // metric name
      1, // inital value of type i64
      Semantics::Counter,
      Unit::new().count(Count::One, 1).unwrap(), // unit with a 'count' dimension of power 1
      "Short text", // short description
      "Long text", // long description
  ).unwrap();
  ```

If we want to create an `f64` metric, we simply pass an `f64` inital value instead

  ```rust
  let mut metric = Metric::new(
      "simple_f64", // metric name
      1.5, // inital value of type f64
      Semantics::Instant,
      Unit::new().count(Time::Sec, 1).unwrap(), // unit with a 'time' dimension of power 1
      "Short text", // short description
      "Long text", // long description
  ).unwrap();
  ```

And similarly for a `String` metric

  ```rust
  let mut metric = Metric::new(
      "simple_string", // metric name
      "Hello, world!".to_string(), // inital value of type String
      Semantics::Discrete,
      Unit::new().unwrap(), // unit with no dimension
      "Short text", // short description
      "Long text", // long description
  ).unwrap();
  ```

The detailed API on singleton metrics can be found [here](https://docs.rs/hornet/0.1.0/hornet/client/metric/struct.Metric.html).

### Instance Metric

An instance metric is similar to a singleton metric in that it is also associated with a primitive valye type, `Unit`, and `Semantics`,
but additionally also holds multiple independent primitive values of the same type. The same type inference rules also hold for instance metrics - the type
of the inital value determines the type of the instance metric.

Before we can create an instance metric, we need to create what's called an
*instance domain*. An instance domain is a set of `String` values that act
as unique identifiers for the multiple independent values of an instance metric. Why have a separate object for this purpose? So that we can reuse the same identifiers as a "domain" for several different but related instance metrics. An example will clear this up.

Suppose we are modeling the fictional [Acme Corporation factory](https://en.wikipedia.org/wiki/Acme_Corporation). Let's assume we have three items that can be manufactured - Anvils, Rockets, and Giant Rubber Bands. Each item is associated with a "count" metric of how many copies have been manufactured so far, and a "time" metric of how much time has been spent manufacturing each item. We can create instance metrics like so

  ```rust
  /* instance domain */
  let indom = Indom::new(
      &["Anvils", "Rockets", "Giant_Rubber_Bands"],
      "Acme products", // short description
      "Most popular products produced by the Acme Corporation" // long description
  ).unwrap();
    
  /* two instance metrics */

  let mut counts = InstanceMetric::new(
      &indom,
      "products.count", // instance metric name
      0, // inital value of type i64
      Semantics::Counter,
      Unit::new().count(Count::One, 1).unwrap(),
      "Acme factory product throughput",
      "Monotonic increasing counter of products produced in the Acme Corporation factory since starting the Acme production application."
  ).unwrap();

  let mut times = InstanceMetric::new(
      &indom,
      "products.time",  // instance metric name
      0.0, // inital value of type f64
      Semantics::Instance,
      Unit::new().time(Time::Sec, 1).unwrap(),
      "Time spent producing products",
      "Machine time spent producing Acme Corporation products."
  ).unwrap();

  ```

Here, our `indom` contains three identifiers - `Anvils`, `Rockets` and `Giant_Rubber_Bands`.
We've created two instance metrics - `counts` of type `i64` and `times` of type `f64` with relevant units and semantics.

The detailed API on instance metrics can be found [here](https://docs.rs/hornet/0.1.0/hornet/client/metric/struct.InstanceMetric.html).

### Updating metrics

So far we've seen how to create metrics with various attributes. Updating their primitive values is pretty simple.

For singleton metrics, the `val(&self) -> &T` method returns a reference to the underlying value, and the
`set_val(&mut self, new_val: T) -> io::Result<()>` method updates the underlying value and
writes to the memory mapped file. The arguments and return values for these methods
are generic over the different primitive types for a metric, and hence are completely type safe.

For instance metrics, the `val(&self, instance: &str) -> Option<&T>` method returns a reference to the primitive value for the given instance identifier, if it exists. The
`set_val(&mut self, instance: &str, new_val: T) -> Option<io::Result<()>>` method updates the primitive value for the given instance identifier, if it exists. These methods are similarly
generic over primitive value types.

## Special metrics

Singleton metrics and instance metrics are powerful and general enough to be used for a wide variety of performance analysis needs. However, for many common applications, simpler metric interfaces would be more appropriate and easy to use. Hence `hornet` includes 6 high-level metrics that are built on top of singleton and instance metrics, and they offer a more specialized and simpler API.

#### Counter

A `Counter` is a singleton metric of type `u64`, `Counter` semantics, and unit of 1 count dimension. It implements the following methods: `up` to increment by one, `inc` to increment
by a delta, `reset` to set count to the inital count, and `val` to return the current count.

  ```rust
  let mut c = Counter::new(
      "counter", // name
      1, // inital value
      "", "" // short and long description strings
  ).unwrap();

  c.up(); // 2
  c.inc(3); // 5
  c.reset(); // 1

  let count = c.val(); // 1
  ```

The [CountVector](https://docs.rs/hornet/0.1.0/hornet/client/metric/struct.CountVector.html) is the instance metric version of the `Counter`. It holds multiple counts each associated with a `String` identifier.

#### Gauge

A `Gauge` is a singleton metric of type `f64`, `Instant` semantics, and unit of 1 count dimension. It implements the following methods: `inc` to increment the gauge by a delta, `dec` to decrement the gauge by a delta, `set` to set the gauge to an arbritrary value, and `val` which returns the current value of the gauge.

  ```rust
  let mut gauge = Gauge::new("gauge", 1.5, "", "").unwrap();
  
  gauge.set(3.0).unwrap(); // 3.0
  gauge.inc(3.0).unwrap(); // 6.0
  gauge.dec(1.5).unwrap(); // 4.5
  gauge.reset().unwrap();  // 1.5
  ```

The [GaugeVector](https://docs.rs/hornet/0.1.0/hornet/client/metric/struct.GaugeVector.html) is the instance metric version of the `Gauge`. It holds multiple gauge values each associated with an identifier.

#### Timer

A `Timer` is a singleton metric of type `i64`, `Instant` semantics, and a user specified time unit. It implements the following methods: `start` starts the timer by recording the current time, `stop` stops the timer by recording the current time
and returns the elapsed time since the last `start`, and `elapsed` returns the
total time elapsed so far between all start and stop pairs.

  ```rust
  let mut timer = Timer::new("timer", Time::MSec, "", "").unwrap();

  timer.start().unwrap();
  let e1 = timer.stop().unwrap();

  timer.start().unwrap();
  let e2 = timer.stop().unwrap();

  let elapsed = timer.elapsed(); // = e1 + e2
  ```

#### Histogram

A `Histogram` is a high dynamic range (HDR) histogram metric which records `u64` data points and exports various statistics about the data. It is implemented using an instance metric of `f64` type and `Instance` semantics. The `Histogram` metric is infact essentially a wrapper around the `Histogram` object from the [hdrsample](https://github.com/jonhoo/hdrsample) crate, and it exports the maximum, minimum, mean and standard deviation statistics to the MMV file.

  ```rust
  let low = 1;
  let high = 100;
  let sigfig = 5;

  let mut hist = Histogram::new(
      "histogram",
      low,
      high,
      sigfig,
      Unit::new().count(Count::One, 1).unwrap(),
      "Simple histogram example", ""
  ).unwrap();

  let range = Range::new(low, high);
  let mut thread_rng = thread_rng();

  for _ in 0..100 {
      hist.record(range.ind_sample(&mut thread_rng)).unwrap();
  }
  ```

Much of the `Histogram` [API](https://docs.rs/hornet/0.1.0/hornet/client/metric/struct.Histogram.html) is largely similar to the [hdrsample API](https://docs.rs/hdrsample/6.0.1/hdrsample/struct.Histogram.html).

### Client

In order to export our metrics to a memory mapped file, we must first create a `Client`

  ```rust
  let client = Client::new("client").unwrap(); // MMV file will be named 'client'
  ```

Now to export metrics, we simply call `export`

  ```rust
  client.export(&mut [&mut metric1, &mut metric2, &mut metric3]);
  ```

If you have a valid PCP installation, the `Client` writes the MMV file to `$PCP_TMP_DIR/mmv/`, and otherwise it writes it to `/tmp/mmv/`.

After metrics are exported through a `Client`, all updates to their primitive values will show up in the MMV file.

### Monitoring metrics

With a valid PCP installation on a machine, metrics can be monitored externally by using the follwing command
  ```bash
  $ pminfo -f mmv._name_
  ```
where `_name_` is the name passed to `Client` while creating it.

Another way to inspect metrics externally is to dump the contents of the MMV file itself. This can be done using a command line tool called `mmvdump` included in `hornet`. After issuing `cargo build` from within the project directory, `mmvdump` can be found built under `target/debug/`.

Usage of `mmvdump` is pretty straightforward

  ```rust
  $ ./mmvdump simple.mmv

  Version    = 1
  Generated  = 1468770536
  TOC count  = 3
  Cluster    = 127
  Process    = 29956
  Flags      = process (0x2)

  TOC[0]: toc offset 40, metrics offset 88 (1 entries)
    [725/88] simple.counter
        type=Int32 (0x0), sem=counter (0x1), pad=0x0
        unit=count (0x100000)
        (no indom)
        shorttext=A Simple Metric
        longtext=This is a simple counter metric to demonstrate the hornet API

  TOC[1]: toc offset 56, values offset 192 (1 entries)
    [725/192] simple.counter = 42

  TOC[2]: toc offset 72, strings offset 224 (2 entries)
    [1/224] A Simple Metric
    [2/480] This is a simple counter metric to demonstrate the hornet API
  ```

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
