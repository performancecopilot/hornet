extern crate hornet; 
extern crate rand;

use hornet::client::Client;
use hornet::client::metric::*;
use rand::{thread_rng, Rng};
use std::thread;
use std::time::Duration;

fn main() {
    
    /* create three singleton metrics */

    let mut color = Metric::new(
        "color",
        String::from("cyan"),
        Semantics::Discrete,
        Unit::new(),
        "Color",
        "",
    ).unwrap();

    let hz = Unit::new().time(Time::Sec, -1).unwrap();
    let mut freq = Metric::new(
        "frequency", // name (max 63 bytes)
        thread_rng().gen::<f64>(), // initial value
        Semantics::Instant, // semantics
        hz, // unit
        "", // optional short description (max 255 bytes)
        "", // optional long description (max 255 bytes)
    ).unwrap();

    let mut photons = Metric::new(
        "photons",
        thread_rng().gen::<u32>(),
        Semantics::Counter,
        Unit::new().count(Count::One, 1).unwrap(),
        "No. of photons",
        "Number of photons emitted by source",
    ).unwrap();

    /* create a client, register the metrics with it, and export them */

    Client::new("physical_metrics").unwrap()
        .begin(0, 0, 3).unwrap()
        .register_metric(&mut freq).unwrap()
        .register_metric(&mut color).unwrap()
        .register_metric(&mut photons).unwrap()
        .export().unwrap();

    /* update metric values */

    color.set_val(String::from("magenta")).unwrap();

    loop {
        freq.set_val(thread_rng().gen::<f64>()).unwrap();
        photons.set_val(thread_rng().gen::<u32>()).unwrap();

        thread::sleep(Duration::from_secs(1));
    }

}
