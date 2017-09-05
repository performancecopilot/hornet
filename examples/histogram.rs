extern crate hornet; 
extern crate rand;

use hornet::client::Client;
use hornet::client::metric::*;
use rand::thread_rng;
use rand::distributions::{IndependentSample, Range};

/*
    For detailed usage and behaviour of the underlying HDR histogram object,
    check out jonhoo's hdrsample crate at https://github.com/jonhoo/hdrsample 
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
        "Simple histogram example", ""
    ).unwrap();

    /* export it to an mmv */

    let client = Client::new("histogram").unwrap();
    client.export(&mut [&mut hist]).unwrap();
    println!("Histogram mapped at {}", client.mmv_path().to_str().unwrap());

    /* record 100 random values */

    let range = Range::new(low, high);
    let mut thread_rng = thread_rng();

    for _ in 0..100 {
        hist.record(range.ind_sample(&mut thread_rng)).unwrap();
    }

    /* record a single random value 100 times */
    
    hist.record_n(range.ind_sample(&mut thread_rng), 100).unwrap();

}
