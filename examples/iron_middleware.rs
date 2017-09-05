extern crate iron;
extern crate hornet;

use std::sync::{Mutex, Arc};
use hornet::client::Client;
use hornet::client::metric::*;
use iron::prelude::*;
use iron::method::Method;
use iron::status;

/*
    this examples demonstrates usage of CountVector metric
    embedded in Iron BeforeMiddleware
*/

static URL: &'static str = "127.0.0.1:8000";

fn method_str(method: &Method) -> String {
    format!("{}", method)
}

fn main() {
    let mut methods_count = CountVector::new(
        "methods_count",
        0,
        &[
            &method_str(&Method::Options),
            &method_str(&Method::Get),
            &method_str(&Method::Post),
            &method_str(&Method::Put),
            &method_str(&Method::Delete),
            &method_str(&Method::Head),
            &method_str(&Method::Trace),
            &method_str(&Method::Connect)
        ],
        "Counts of recieved HTTP request methods", ""
    ).unwrap();

    let client = Client::new("localhost.methods").unwrap();
    client.export(&mut [&mut methods_count]).unwrap();

    let mut chain = Chain::new(|_: &mut Request| {
        Ok(Response::with((status::Ok, "Hello World!")))
    });

    let mutex = Mutex::new(methods_count);
    let arc = Arc::new(mutex);

    chain.link_before(move |req: &mut Request| {
        match &req.method {
            &Method::Extension(_) => {},
            _ => {
                let mut counter = arc.lock().unwrap();
                counter.up(&method_str(&req.method)).unwrap().unwrap();
            }
        }
        Ok(())
    });

    println!("Listening on http://{}", URL);
    println!("Counter mapped at {}", client.mmv_path().to_str().unwrap());

    Iron::new(chain).http(URL).unwrap();
}
