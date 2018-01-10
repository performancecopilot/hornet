extern crate hornet;
extern crate curl;

use hornet::client::Client;
use hornet::client::metric::*;
use curl::easy::Easy;

/*
    this example uses the Timer metric to measure time spent
    downloading the zipped linux kernel source using libcurl bindings
*/

const URL: &'static str = "https://codeload.github.com/torvalds/linux/zip/master";

fn main() {

    let mut timer = Timer::new(
        "time",
        Time::Sec,
        "Time elapsed downloading", "").unwrap();

    let mut bytes = Metric::new(
        "bytes",
        0,
        Semantics::Discrete,
        Unit::new().space(Space::Byte, 1).unwrap(),
        "Bytes downloaded so far", "").unwrap();

    let client = Client::new("download").unwrap();
    client.export(&mut [&mut timer, &mut bytes]).unwrap();

    let mut easy = Easy::new();
    easy.url(URL).unwrap();

    easy.progress(true).unwrap();
    easy.progress_function(move |_, bytes_downloaded, _, _| {
        timer.stop().ok();
        timer.start().ok();
        bytes.set_val(bytes_downloaded as u64).unwrap();
        true
    }).unwrap();

    println!("Downloading from {}", URL);
    println!("Progress mapped at {}", client.mmv_path().to_str().unwrap());

    easy.perform().unwrap();

}
