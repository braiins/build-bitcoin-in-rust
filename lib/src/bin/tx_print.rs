use btclib::types::Transaction;
use btclib::util::Saveable;

use std::env;
use std::fs::File;
use std::process::exit;

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprintln!("Usage: tx_print <tx_file>");
        exit(1);
    };

    if let Ok(file) = File::open(path) {
        let tx = Transaction::load(file)
            .expect("Failed to load transaction");
        println!("{:#?}", tx);
    }
}
