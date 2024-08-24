use btclib::crypto::PrivateKey;
use btclib::types::{Transaction, TransactionOutput};
use btclib::util::Saveable;

use uuid::Uuid;

use std::env;
use std::process::exit;

fn main() {
    let path = if let Some(arg) = env::args().nth(1) {
        arg
    } else {
        eprintln!("Usage: tx_gen <tx_file>");
        exit(1);
    };

    let private_key = PrivateKey::new_key();

    let transaction = Transaction::new(
        vec![],
        vec![TransactionOutput {
            unique_id: Uuid::new_v4(),
            value: btclib::INITIAL_REWARD * 10u64.pow(8),
            pubkey: private_key.public_key(),
        }],
    );

    transaction
        .save_to_file(path)
        .expect("Failed to save transaction");
}
