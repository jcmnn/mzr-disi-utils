//! This example queries a VIN using a PassThru device

use anyhow::Result;
use obd::{PassThruIsoTp, Uds};
use std::fs;

use mzr::{MzrBus};

use clap::clap_app;
use indicatif::{ProgressBar, ProgressStyle};

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
    let vin = driver.query_vin(0x7e0).unwrap();
    println!("VIN: {}", vin);

    // Query trouble codes
    for code in driver.query_trouble_codes(0x7e0).unwrap().iter() {
        println!("{}", code);
    }
}