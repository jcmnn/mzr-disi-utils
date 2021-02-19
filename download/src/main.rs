//! This example queries a VIN using a PassThru device

use anyhow::Result;
use obd::{PassThruIsoTp, Uds};
use std::fs;

use mzr::{DownloadState, Downloader, MzrBus};

use clap::clap_app;
use indicatif::{ProgressBar, ProgressStyle};

pub fn main() {
    let matches = clap_app!(myapp =>
        (version: "1.0")
        (author: "Jacob Manning <jjacob.manning@gmail.com>")
        (about: "Downloads ROM from an MZR-DISI ECU")
        (@arg passthru: -p --passthru +takes_value "PassThru device to use when connecting to the ECU")
        (@arg model: -m --model +takes_value "Vehicle model")
        (@arg OUTPUT: "Output file (defaults to <vin>.bin)")
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

    // Query trouble codes
    /*for code in driver.query_trouble_codes(0x7e0).unwrap().iter() {
        println!("{}", code);
    }*/

    // Create PassThru connection
    let mut driver = PassThruIsoTp::new(&d, 500000, 10000).unwrap();
    // isotp.set_filter(0x7e0, 0x7e8);
    let vin = driver.query_vin(0x7e0).unwrap();
    println!("VIN: {}", vin);

    // Authenticate and download
    let mut downloader = Downloader::new(&mut driver);

    // Create progress bar
    let pb = ProgressBar::new(downloader.total_size() as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .progress_chars("#>-"));

    downloader.start().unwrap();
    while let DownloadState::InProgress(downloaded) = downloader.step().unwrap() {
        pb.set_position(downloaded as u64);
    }
    pb.finish_with_message("downloaded");
    let data = downloader.take_data();

    // Get output path
    let output_path = matches
        .value_of("OUTPUT")
        .map(|s| s.to_string())
        .unwrap_or_else(|| vin + ".bin");

    fs::write(&output_path, &data).unwrap();
    println!("Downloaded to {}", output_path);
}
