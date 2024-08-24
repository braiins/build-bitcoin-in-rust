use anyhow::Result;
use crossbeam_skiplist::SkipMap;
use kanal::Sender;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use btclib::crypto::{PrivateKey, PublicKey};
use btclib::network::Message;
use btclib::types::{Transaction, TransactionOutput};
use btclib::util::Saveable;

/// Represent a key pair with paths to public and private keys.
#[derive(Serialize, Deserialize, Clone)]
pub struct Key {
    pub public: PathBuf,
    pub private: PathBuf,
}

/// Represent a loaded key pair with actual public and private keys.
#[derive(Clone)]
struct LoadedKey {
    public: PublicKey,
    private: PrivateKey,
}

/// Represent a recipient with a name and a path to their public key.
#[derive(Serialize, Deserialize, Clone)]
pub struct Recipient {
    pub name: String,
    pub key: PathBuf,
}

/// Represent a loaded recipient with their actual public key.
#[derive(Clone)]
pub struct LoadedRecipient {
    pub key: PublicKey,
}

impl Recipient {
    /// Load the recipient's public key from file.
    pub fn load(&self) -> Result<LoadedRecipient> {
        debug!("Loading recipient key from: {:?}", self.key);
        let key = PublicKey::load_from_file(&self.key)?;
        Ok(LoadedRecipient { key })
    }
}

/// Define the type of fee calculation.
#[derive(Serialize, Deserialize, Clone)]
pub enum FeeType {
    Fixed,
    Percent,
}

/// Configure the fee calculation.
#[derive(Serialize, Deserialize, Clone)]
pub struct FeeConfig {
    pub fee_type: FeeType,
    pub value: f64,
}

/// Store the configuration for the Core.
#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub my_keys: Vec<Key>,
    pub contacts: Vec<Recipient>,
    pub default_node: String,
    pub fee_config: FeeConfig,
}

/// Store and manage Unspent Transaction Outputs (UTXOs).
#[derive(Clone)]
struct UtxoStore {
    my_keys: Vec<LoadedKey>,
    utxos:
        Arc<SkipMap<PublicKey, Vec<(bool, TransactionOutput)>>>,
}

impl UtxoStore {
    /// Create a new UtxoStore.
    fn new() -> Self {
        UtxoStore {
            my_keys: Vec::new(),
            utxos: Arc::new(SkipMap::new()),
        }
    }

    /// Add a new key to the UtxoStore.
    fn add_key(&mut self, key: LoadedKey) {
        debug!("Adding key to UtxoStore: {:?}", key.public);
        self.my_keys.push(key);
    }
}

/// Represent the core functionality of the wallet.
pub struct Core {
    pub config: Config,
    utxos: UtxoStore,
    pub tx_sender: Sender<Transaction>,
    pub stream: Mutex<TcpStream>,
}

impl Core {
    /// Create a new Core instance.
    fn new(
        config: Config,
        utxos: UtxoStore,
        stream: TcpStream,
    ) -> Self {
        let (tx_sender, _) = kanal::bounded(10);
        Core {
            config,
            utxos,
            tx_sender,
            stream: Mutex::new(stream),
        }
    }

    /// Load the Core from a configuration file.
    pub async fn load(config_path: PathBuf) -> Result<Self> {
        info!("Loading core from config: {:?}", config_path);
        let config: Config =
            toml::from_str(&fs::read_to_string(&config_path)?)?;
        let mut utxos = UtxoStore::new();

        let stream =
            TcpStream::connect(&config.default_node).await?;

        // Load keys from config
        for key in &config.my_keys {
            debug!("Loading key pair: {:?}", key.public);
            let public = PublicKey::load_from_file(&key.public)?;
            let private =
                PrivateKey::load_from_file(&key.private)?;
            utxos.add_key(LoadedKey { public, private });
        }

        Ok(Core::new(config, utxos, stream))
    }

    /// Fetch UTXOs from the node for all loaded keys.
    pub async fn fetch_utxos(&self) -> Result<()> {
        debug!(
            "Fetching UTXOs from node: {}",
            self.config.default_node
        );

        for key in &self.utxos.my_keys {
            let message =
                Message::FetchUTXOs(key.public.clone());
            message
                .send_async(&mut *self.stream.lock().await)
                .await?;

            if let Message::UTXOs(utxos) =
                Message::receive_async(
                    &mut *self.stream.lock().await,
                )
                .await?
            {
                debug!(
                    "Received {} UTXOs for key: {:?}",
                    utxos.len(),
                    key.public
                );
                // Replace the entire UTXO set for this key
                self.utxos.utxos.insert(
                    key.public.clone(),
                    utxos
                        .into_iter()
                        .map(|(output, marked)| (marked, output))
                        .collect(),
                );
            } else {
                error!("Unexpected response from node");
                return Err(anyhow::anyhow!(
                    "Unexpected response from node"
                ));
            }
        }

        info!("UTXOs fetched successfully");
        Ok(())
    }

    /// Send a transaction to the node.
    pub async fn send_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<()> {
        debug!(
            "Sending transaction to node: {}",
            self.config.default_node
        );
        let message = Message::SubmitTransaction(transaction);
        message
            .send_async(&mut *self.stream.lock().await)
            .await?;
        info!("Transaction sent successfully");
        Ok(())
    }

    /// Prepare and send a transaction asynchronously.
    pub fn send_transaction_async(
        &self,
        recipient: &str,
        amount: u64,
    ) -> Result<()> {
        info!(
            "Preparing to send {} satoshis to {}",
            amount, recipient
        );
        let recipient_key = self
            .config
            .contacts
            .iter()
            .find(|r| r.name == recipient)
            .ok_or_else(|| {
                anyhow::anyhow!("Recipient not found")
            })?
            .load()?
            .key;

        let transaction =
            self.create_transaction(&recipient_key, amount)?;

        debug!("Sending transaction asynchronously");
        self.tx_sender.send(transaction)?;
        Ok(())
    }

    /// Get the current balance of all UTXOs.
    pub fn get_balance(&self) -> u64 {
        let balance = self
            .utxos
            .utxos
            .iter()
            .map(|entry| {
                entry
                    .value()
                    .iter()
                    .map(|utxo| utxo.1.value)
                    .sum::<u64>()
            })
            .sum();
        debug!("Current balance: {} satoshis", balance);
        balance
    }

    /// Create a new transaction.
    pub fn create_transaction(
        &self,
        recipient: &PublicKey,
        amount: u64,
    ) -> Result<Transaction> {
        debug!(
            "Creating transaction for {} satoshis to {:?}",
            amount, recipient
        );
        let fee = self.calculate_fee(amount);
        let total_amount = amount + fee;

        let mut inputs = Vec::new();
        let mut input_sum = 0;

        for entry in self.utxos.utxos.iter() {
            let pubkey = entry.key();
            let utxos = entry.value();

            for (marked, utxo) in utxos.iter() {
                if *marked {
                    continue;
                } // Skip marked UTXOs
                if input_sum >= total_amount {
                    break;
                }
                inputs.push(btclib::types::TransactionInput {
                    prev_transaction_output_hash: utxo.hash(),
                    signature:
                        btclib::crypto::Signature::sign_output(
                            &utxo.hash(),
                            &self
                                .utxos
                                .my_keys
                                .iter()
                                .find(|k| k.public == *pubkey)
                                .unwrap()
                                .private,
                        ),
                });
                input_sum += utxo.value;
            }
            if input_sum >= total_amount {
                break;
            }
        }

        if input_sum < total_amount {
            error!("Insufficient funds: have {} satoshis, need {} satoshis", input_sum, total_amount);
            return Err(anyhow::anyhow!("Insufficient funds"));
        }

        let mut outputs = vec![TransactionOutput {
            value: amount,
            unique_id: uuid::Uuid::new_v4(),
            pubkey: recipient.clone(),
        }];

        if input_sum > total_amount {
            outputs.push(TransactionOutput {
                value: input_sum - total_amount,
                unique_id: uuid::Uuid::new_v4(),
                pubkey: self.utxos.my_keys[0].public.clone(),
            });
        }

        info!("Transaction created successfully");
        Ok(Transaction::new(inputs, outputs))
    }

    /// Calculate the fee for a transaction.
    fn calculate_fee(&self, amount: u64) -> u64 {
        let fee = match self.config.fee_config.fee_type {
            FeeType::Fixed => {
                self.config.fee_config.value as u64
            }
            FeeType::Percent => {
                (amount as f64 * self.config.fee_config.value
                    / 100.0) as u64
            }
        };
        debug!("Calculated fee: {} satoshis", fee);
        fee
    }
}
