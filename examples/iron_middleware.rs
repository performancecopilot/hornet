extern crate iron;
extern crate hornet;

use std::sync::Mutex;
use hornet::client::Client;
use hornet::client::metric::*;
use iron::prelude::*;
use iron::middleware::BeforeMiddleware;
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

struct MethodCounter {
    pub metric: Mutex<CountVector>
}

impl MethodCounter {
    fn new() -> Self {
        let metric = CountVector::new(
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
            "Counts of recieved HTTP request methods", "").unwrap();
        
        MethodCounter {
            metric: Mutex::new(metric)
        }
    }
}

impl BeforeMiddleware for MethodCounter {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        match &req.method {
            &Method::Extension(_) => {},
            _ => {
                let mut counter = self.metric.lock().unwrap();
                counter.up(&method_str(&req.method)).unwrap().unwrap();
            }
        }
        Ok(())
    }

    fn catch(&self, _: &mut Request, _: IronError) -> IronResult<()> {
        Ok(())
    }
}

fn main() {
    let method_counter = MethodCounter::new();

    let client = Client::new("localhost.methods").unwrap();
    {
        let mut metric = method_counter.metric.lock().unwrap();
        client.export(&mut [&mut *metric]).unwrap();
    }

    let mut chain = Chain::new(|_: &mut Request| {
        Ok(Response::with((status::Ok, "Hello World!")))
    });
    chain.link_before(method_counter);

    println!("Listening on http://{}", URL);
    println!("Counter mapped at {}", client.mmv_path().to_str().unwrap());

    Iron::new(chain).http(URL).unwrap();
}
