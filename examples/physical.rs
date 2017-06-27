extern crate hornet; 
extern crate rand;

use hornet::client::Client;
use hornet::client::metric::*;
use rand::{thread_rng, Rng};

fn main() {
    
    /* create three singleton metrics */

    let hz = Unit::new().time(Time::Sec, -1).unwrap();
    let mut freq = Metric::new(
        "frequency", // name
        0, // item ID
        Semantics::Instant, // semantics
        hz, // unit
        thread_rng().gen::<f64>(), // initial value
        "", // optional short description (max 255 bytes)
        "", // optional long description (max 255 bytes)
    ).unwrap();

    let mut color = Metric::new(
        "color",
        0,
        Semantics::Discrete,
        Unit::new(),
        String::from("cyan"),
        "Color",
        "",
    ).unwrap();

    let mut photons = Metric::new(
        "photons",
        0,
        Semantics::Counter,
        Unit::new().count(Count::One, 1).unwrap(),
        thread_rng().gen::<u32>(),
        "No. of photons",
        "Number of photons emitted by source",
    ).unwrap();

    /* create a client, register the metrics with it, and export them */

    Client::new("physical_metrics").unwrap()
        .begin(3).unwrap()
        .register_metric(&mut freq).unwrap()
        .register_metric(&mut color).unwrap()
        .register_metric(&mut photons).unwrap()
        .export().unwrap();

    /* update metric values */

    freq.set_val(thread_rng().gen::<f64>()).unwrap();

    color.set_val(String::from("magenta")).unwrap();

    photons.set_val(thread_rng().gen::<u32>()).unwrap();

}
