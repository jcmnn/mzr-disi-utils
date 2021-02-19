//! This example queries a VIN using a PassThru device

use anyhow::Result;
use obd::{PassThruIsoTp, Uds};
use std::fs;

use mzr::{MzrBus};

use clap::clap_app;
use indicatif::{ProgressBar, ProgressStyle};

use std::time::{Duration, Instant};

pub fn main() {
    let matches = clap_app!(myapp =>
        (version: "1.0")
        (author: "Jacob Manning <jjacob.manning@gmail.com>")
        (about: "Queries information from an MZR-DISI ECU")
        (@arg passthru: -p --passthru +takes_value "PassThru device to use when connecting to the ECU")
        (@arg model: -m --model +takes_value "Vehicle model")
    )
    .get_matches();

    // Get a list of interfaces
    let device = match j2534::drivers().unwrap().into_iter().next() {
        Some(device) => device,
        None => {
            println!("No J2534 interfaces found");
            return;
        }
    };

    println!("Opening interface '{}'", device.name);
    let i = j2534::Interface::new(&device.path).unwrap();
    // Open any connected device
    let d = i.open_any().unwrap();
    // Get version information
    let version_info = d.read_version().unwrap();
    println!("{:#?}", version_info);


    // Create PassThru connection
    let mut driver = PassThruIsoTp::new(&d, 500000, 15000).unwrap();
    // isotp.set_filter(0x7e0, 0x7e8);

    let start_time = Instant::now();
    for i in 0..100 {
        let response = driver.query_uds(0x7e0, 0x22, &[0, 0, 0, 1, 0, 2]).unwrap(); //, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0, 9, 0, 10, 0, 11, 0, 12, 0, 13, 0, 14, 0, 15, 0, 16, 0, 17, 0, 18, 0, 19, 0, 20]).unwrap();
    }
    println!("PID/s: {}", 3.0 * 100.0 / start_time.elapsed().as_secs_f64());
}