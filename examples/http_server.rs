extern crate hornet; 
extern crate hyper;
extern crate futures;

use std::sync::{Mutex, Arc};
use hornet::client::Client;
use hornet::client::metric::*;
use futures::future::FutureResult;
use hyper::header::{ContentLength, ContentType};
use hyper::{Get, StatusCode};
use hyper::server::{Http, Service, Request, Response};

/*
    records count of HTTP GET requests on localhost:8000

    this example also shows how to safely read and update
    a metric concurrently
*/

static URL: &'static str = "127.0.0.1:8000";

struct HTTPCounterService {
    arc: Arc<Mutex<Counter>>
}

impl Service for HTTPCounterService {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = FutureResult<Response, hyper::Error>;

    fn call(&self, req: Request) -> Self::Future {
        futures::future::ok(match (req.method(), req.path()) {
            (&Get, "/") => {

                let mut counter = self.arc.lock().unwrap();

                /* increase the counter value by one */
                counter.up().unwrap();

                let body = format!("HTTP GET count = {}", counter.val());
                Response::new()
                    .with_header(ContentLength(body.len() as u64))
                    .with_header(ContentType::plaintext())
                    .with_body(body)

            },
            _ => {
                Response::new()
                    .with_status(StatusCode::NotFound)
            }
        })
    }

}

fn main() {

	/* create a counter metric */

	let mut counter = Counter::new(
        "get",
        0, // initial value
        "GET request count", // short description
        &format!("Count of GET requests on http://{}/", URL) // long description
    ).unwrap();

    /* export it to an mmv */

    let client = Client::new("localhost.http").unwrap();
    client.export(&mut [&mut counter]).unwrap();

    /* 
        since the counter could be updated concurrently, wrap it
        in a mutex. to have shared ownership of the mutex itself,
        wrap it in an atomic reference counting pointer
    */
     
    let mutex = Mutex::new(counter);
    let arc = Arc::new(mutex);

    /* create and run the server */

    let addr = URL.parse().unwrap();
    let server = Http::new().bind(&addr, move || {
        Ok(HTTPCounterService {
            arc: arc.clone()
        })
    }).unwrap();

    println!("Listening on http://{}", server.local_addr().unwrap());
    println!("Counter mapped at {}", client.mmv_path().to_str().unwrap());

    server.run().unwrap();    

}
