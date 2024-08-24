use serde::{Deserialize, Serialize};

use std::fs::File;
use std::io::{Read, Result as IoResult, Write};
use std::path::Path;

use crate::sha256::Hash;
use crate::types::Transaction;

#[derive(
    Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq,
)]

pub struct MerkleRoot(Hash);

impl MerkleRoot {
    // calculate the merkle root of a block's transactions
    pub fn calculate(
        transactions: &[Transaction],
    ) -> MerkleRoot {
        let mut layer: Vec<Hash> = vec![];

        for transaction in transactions {
            layer.push(Hash::hash(transaction));
        }

        while layer.len() > 1 {
            let mut new_layer = vec![];

            for pair in layer.chunks(2) {
                let left = pair[0];
                // if there is no right, use the left hash again
                let right = pair.get(1).unwrap_or(&pair[0]);
                new_layer.push(Hash::hash(&[left, *right]));
            }

            layer = new_layer;
        }

        MerkleRoot(layer[0])
    }
}

pub trait Saveable
where
    Self: Sized,
{
    fn load<I: Read>(reader: I) -> IoResult<Self>;
    fn save<O: Write>(&self, writer: O) -> IoResult<()>;

    fn save_to_file<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> IoResult<()> {
        let file = File::create(&path)?;
        self.save(file)
    }

    fn load_from_file<P: AsRef<Path>>(
        path: P,
    ) -> IoResult<Self> {
        let file = File::open(&path)?;
        Self::load(file)
    }
}
