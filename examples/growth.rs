extern crate hornet; 
extern crate rand;

use hornet::client::Client;
use hornet::client::metric::*;
use std::thread;
use std::time::Duration;

/* this examples demonstrates use of Counter and GaugeVector */

fn main() {

    let mut n = Counter::new(
        "n",
        0,
        "Input to various functions", "").unwrap();

	let mut f_n = GaugeVector::new(
        "functions",
        0.0,
        &["log2(n)", "nlog2(n)", "n^2", "n^3", "n^4", "2^n", "10^n"],
        "Growth of various functions", "").unwrap();

    let client = Client::new("growth").unwrap();
    client.export(&mut [&mut n, &mut f_n]).unwrap();
    println!("Values mapped at {}", client.mmv_path().to_str().unwrap());

    for _ in 0..60 {

        let val = n.val() as f64;

        f_n.set("log2(n)", val.log2()).unwrap().unwrap();
        f_n.set("nlog2(n)", val*val.log2()).unwrap().unwrap();
        f_n.set("n^2", val.powi(2)).unwrap().unwrap();
        f_n.set("n^3", val.powi(3)).unwrap().unwrap();
        f_n.set("n^4", val.powi(4)).unwrap().unwrap();
        f_n.set("2^n", val.exp2()).unwrap().unwrap();
        f_n.set("10^n", 10_f64.powf(val)).unwrap().unwrap();

        n.up().unwrap();

        thread::sleep(Duration::from_secs(1));

    }

}
