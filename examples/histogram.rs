use hornet::client::metric::*;
use hornet::client::Client;
use rand::Rng;

/*
    For detailed usage and behaviour of the underlying HDR histogram object,
    check out the hdrhistogram crate at https://github.com/HdrHistogram/HdrHistogram_rust
*/

fn main() {
    /* pick parameters for the histogram */

    let low = 1;
    let high = 100;
    let significant_figures = 5;

    /* create a histogram metric */

    let mut hist = Histogram::new(
        "histogram",
        low,
        high,
        significant_figures,
        Unit::new().count(Count::One, 1).unwrap(),
        "Simple histogram example",
        "",
    )
    .unwrap();

    /* export it to an mmv */

    let client = Client::new("histogram").unwrap();
    client.export(&mut [&mut hist]).unwrap();
    println!(
        "Histogram mapped at {}",
        client.mmv_path().to_str().unwrap()
    );

    /* record 100 random values */

    let mut rng = rand::thread_rng();

    for _ in 0..100 {
        hist.record(rng.gen_range(low..high)).unwrap();
    }

    /* record a single random value 100 times */

    hist.record_n(rng.gen_range(low..high), 100).unwrap();
}
