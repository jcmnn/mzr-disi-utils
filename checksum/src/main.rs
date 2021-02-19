use std::convert::TryFrom;
use std::fs;
use std::num::Wrapping;

use clap::clap_app;

fn compute_checksum(data: &[u8]) -> u32 {
    let mut sum = Wrapping(0_u32);

    for chunk in data.chunks(4) {
        sum = sum + Wrapping(u32::from_be_bytes(<&[u8; 4]>::try_from(chunk).unwrap().to_owned()));
    }

    sum.0
}

/// Returns true if the checksum was corrected
fn correct_checksum(data: &mut [u8], target: u32) -> bool {
    // Zero correction region
    data[0] = 0;
    data[1] = 0;
    data[2] = 0;
    data[3] = 0;

    let sum = compute_checksum(&data);
    let correction: u32 = (Wrapping(target) - Wrapping(sum)).0;
    &mut data[0..4].copy_from_slice(&correction.to_be_bytes());

    compute_checksum(&data) == target
}

pub fn main() {
    let matches = clap_app!(myapp =>
        (version: "1.0")
        (author: "Jacob Manning <jjacob.manning@gmail.com>")
        (about: "Verifies and corrects checksums for MZR-DISI ROMs")
        (@arg correct: --correct "Corrects checksum. This operation modifies the input file")
        (@arg model: -m --model +takes_value "Vehicle model")
        (@arg INPUT: +required "Input file")
    )
    .get_matches();

    let path = matches.value_of("INPUT").unwrap();
    let mut data = fs::read(path).unwrap();

    let offset = 0x48000;
    let end = 0x100000;
    let target = 0x5AA55AA5;
    if data.len() != end {
        println!("Input file has invalid size (expected a 1MiB ROM file).");
        return;
    }

    let checksum = compute_checksum(&data[offset..end]);
    println!("Checksum: {:X}\tTarget: {:X}", checksum, target);
    if checksum == target {
        println!("Checksum is correct!");
    } else {
        if matches.is_present("correct") {
            if correct_checksum(&mut data[offset..end], target) {
                fs::write(path, data).unwrap();
                println!("Corrected checksum! File saved as {}", path);
            } else {
                println!("Failed to correct checksum");
            }
        } else {
            println!("Checksum is incorrect! Correct it with --correct");
        }
    }
}
