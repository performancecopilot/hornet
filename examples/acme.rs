extern crate hornet; 
extern crate rand;

use hornet::client::Client;
use hornet::client::metric::*;
use rand::random;
use std::thread;
use std::time::Duration;

fn main() {

    let products = ["Anvils", "Rockets", "Giant_Rubber_Bands"];
    let indom = Indom::new(
        &products,
        "Acme products",
        "Most popular products produced by the Acme Corporation"
    );
    
    /* create three instance metrics */

    let mut counts = InstanceMetric::new(
        &indom,
        "products.count",
        0,
        Semantics::Counter,
        Unit::new().count(Count::One, 1).unwrap(),
        "Acme factory product throughput",
        "Monotonic increasing counter of products produced in the Acme Corporation\nfactory since starting the Acme production application. Quality guaranteed."
    ).unwrap();

    let sec_unit = Unit::new().time(Time::Sec, 1).unwrap();

    let mut times = InstanceMetric::new(
        &indom,
        "products.time",
        0,
        Semantics::Counter,
        sec_unit,
        "Machine time spent producing Acme products",
        "Machine time spent producing Acme Corporation products. Does not include\ntime in queues waiting for production machinery."
    ).unwrap();

    let mut queue_times = InstanceMetric::new(
        &indom,
        "products.queuetime",
        0,
        Semantics::Counter,
        sec_unit,
        "Queued time while producing Acme products",
        "Time spent in the queue waiting to build Acme Corporation products,\nwhile some other Acme product was being built instead of this one."
    ).unwrap();

    /* create a client, register the metrics with it, and export them */

    Client::new("acme").unwrap()
        .begin_all(1, 3, 3, 0).unwrap()
        .register_instance_metric(&mut counts).unwrap()
        .register_instance_metric(&mut times).unwrap()
        .register_instance_metric(&mut queue_times).unwrap()
        .export().unwrap();

    /* update metrics */

    loop {
        let rnd_idx = random::<usize>() % products.len();
        let product = products[rnd_idx];
        let working_time = random::<u64>() % 3;
        thread::sleep(Duration::from_secs(working_time));

        let count = counts.val(product).unwrap();
        counts.set_val(product, count + 1).unwrap().unwrap();

        let time = times.val(product).unwrap();
        times.set_val(product, time + 1).unwrap().unwrap();

        for i in 0..products.len() {
            if i != rnd_idx {
                let queued_product = products[i];

                let queue_time = queue_times.val(queued_product).unwrap();
                queue_times.set_val(queued_product, queue_time + 1).unwrap().unwrap();
            }
        }
    }
}
