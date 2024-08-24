use anyhow::{anyhow, Result};
use btclib::crypto::PublicKey;
use btclib::network::Message;
use btclib::types::Block;
use btclib::util::Saveable;
use clap::Parser;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    address: String,
    #[arg(short, long)]
    public_key_file: String,
}

struct Miner {
    public_key: PublicKey,
    stream: Mutex<TcpStream>,
    current_template: Arc<std::sync::Mutex<Option<Block>>>,
    mining: Arc<AtomicBool>,
    mined_block_sender: flume::Sender<Block>,
    mined_block_receiver: flume::Receiver<Block>,
}

impl Miner {
    async fn new(
        address: String,
        public_key: PublicKey,
    ) -> Result<Self> {
        let stream = TcpStream::connect(&address).await?;
        let (mined_block_sender, mined_block_receiver) =
            flume::unbounded();
        Ok(Self {
            public_key,
            stream: Mutex::new(stream),
            current_template: Arc::new(std::sync::Mutex::new(
                None,
            )),
            mining: Arc::new(AtomicBool::new(false)),
            mined_block_sender,
            mined_block_receiver,
        })
    }

    async fn run(&self) -> Result<()> {
        self.spawn_mining_thread();

        let mut template_interval =
            interval(Duration::from_secs(5));

        loop {
            let receiver_clone =
                self.mined_block_receiver.clone();

            tokio::select! {
                _ = template_interval.tick() => {
                    self.fetch_and_validate_template().await?;
                }
                Ok(mined_block) = receiver_clone.recv_async() => {
                    self.submit_block(mined_block).await?;
                }
            }
        }
    }

    fn spawn_mining_thread(&self) -> thread::JoinHandle<()> {
        let template = self.current_template.clone();
        let mining = self.mining.clone();
        let sender = self.mined_block_sender.clone();

        thread::spawn(move || loop {
            if mining.load(Ordering::Relaxed) {
                if let Some(mut block) =
                    template.lock().unwrap().clone()
                {
                    println!(
                        "Mining block with target: {}",
                        block.header.target
                    );
                    if block.header.mine(2_000_000) {
                        println!(
                            "Block mined: {}",
                            block.hash()
                        );
                        sender.send(block).expect(
                            "Failed to send mined block",
                        );
                        mining.store(false, Ordering::Relaxed);
                    }
                }
            }
            thread::yield_now();
        })
    }

    async fn fetch_and_validate_template(&self) -> Result<()> {
        if !self.mining.load(Ordering::Relaxed) {
            self.fetch_template().await?;
        } else {
            self.validate_template().await?;
        }
        Ok(())
    }

    async fn fetch_template(&self) -> Result<()> {
        println!("Fetching new template");
        let message =
            Message::FetchTemplate(self.public_key.clone());

        let mut stream_lock = self.stream.lock().await;
        message.send_async(&mut *stream_lock).await?;
        drop(stream_lock);

        let mut stream_lock = self.stream.lock().await;
        match Message::receive_async(&mut *stream_lock).await? {
            Message::Template(template) => {
                drop(stream_lock);
                println!("Received new template with target: {}", template.header.target);
                *self.current_template.lock().unwrap() = Some(template);
                self.mining.store(true, Ordering::Relaxed);
                Ok(())
            }
            _ => Err(anyhow!("Unexpected message received when fetching template")),
        }
    }

    async fn validate_template(&self) -> Result<()> {
        if let Some(template) =
            self.current_template.lock().unwrap().clone()
        {
            let message = Message::ValidateTemplate(template);
            let mut stream_lock = self.stream.lock().await;
            message.send_async(&mut *stream_lock).await?;
            drop(stream_lock);

            let mut stream_lock = self.stream.lock().await;
            match Message::receive_async(&mut *stream_lock).await? {
                Message::TemplateValidity(valid) => {
                    drop(stream_lock);
                    if !valid {
                        println!("Current template is no longer valid");
                        self.mining.store(false, Ordering::Relaxed);
                    } else {
                        println!("Current template is still valid");
                    }
                    Ok(())
                }
                _ => Err(anyhow!("Unexpected message received when validating template")),
            }
        } else {
            Ok(())
        }
    }

    async fn submit_block(&self, block: Block) -> Result<()> {
        println!("Submitting mined block");
        let message = Message::SubmitTemplate(block);
        let mut stream_lock = self.stream.lock().await;
        message.send_async(&mut *stream_lock).await?;
        self.mining.store(false, Ordering::Relaxed);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let public_key =
        PublicKey::load_from_file(&cli.public_key_file)
            .map_err(|e| {
                anyhow!("Error reading public key: {}", e)
            })?;

    let miner = Miner::new(cli.address, public_key).await?;
    miner.run().await
}
