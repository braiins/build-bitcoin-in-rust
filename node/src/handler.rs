use btclib::sha256::Hash;
use chrono::Utc;
use uuid::Uuid;

use tokio::net::TcpStream;

use btclib::network::Message;
use btclib::types::{
    Block, BlockHeader, Transaction, TransactionOutput,
};
use btclib::util::MerkleRoot;

pub async fn handle_connection(mut socket: TcpStream) {
    loop {
        // read a message from the socket
        let message = match Message::receive_async(&mut socket)
            .await
        {
            Ok(message) => message,
            Err(e) => {
                println!("invalid message from peer: {e}, closing that connection");
                return;
            }
        };

        use btclib::network::Message::*;
        match message {
            UTXOs(_) | Template(_) | Difference(_)
            | TemplateValidity(_) | NodeList(_) => {
                println!(
                    "I am neither a miner nor a \
                          wallet! Goodbye"
                );
                return;
            }
            FetchBlock(height) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let Some(block) = blockchain
                    .blocks()
                    .nth(height as usize)
                    .cloned()
                else {
                    return;
                };

                let message = NewBlock(block);
                message.send_async(&mut socket).await.unwrap();
            }
            DiscoverNodes => {
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();
                let message = NodeList(nodes);
                message.send_async(&mut socket).await.unwrap();
            }
            AskDifference(height) => {
                let blockchain = crate::BLOCKCHAIN.read().await;
                let count = blockchain.block_height() as i32
                    - height as i32;
                let message = Difference(count);
                message.send_async(&mut socket).await.unwrap();
            }
            FetchUTXOs(key) => {
                println!("received request to fetch UTXOs");
                let blockchain = crate::BLOCKCHAIN.read().await;

                let utxos = blockchain
                    .utxos()
                    .iter()
                    .filter(|(_, (_, txout))| {
                        txout.pubkey == key
                    })
                    .map(|(_, (marked, txout))| {
                        (txout.clone(), *marked)
                    })
                    .collect::<Vec<_>>();

                let message = UTXOs(utxos);
                message.send_async(&mut socket).await.unwrap();
            }

            NewBlock(block) => {
                let mut blockchain =
                    crate::BLOCKCHAIN.write().await;
                println!("received new block");

                if blockchain.add_block(block).is_err() {
                    println!("block rejected");
                }
            }
            NewTransaction(tx) => {
                let mut blockchain =
                    crate::BLOCKCHAIN.write().await;

                println!("received transaction from friend");

                if blockchain.add_to_mempool(tx).is_err() {
                    println!("transaction rejected, closing connection");
                    return;
                }
            }
            ValidateTemplate(block_template) => {
                let blockchain = crate::BLOCKCHAIN.read().await;

                let status =
                    block_template.header.prev_block_hash
                        == blockchain
                            .blocks()
                            .last()
                            .map(|last_block| last_block.hash())
                            .unwrap_or(Hash::zero());

                let message = TemplateValidity(status);
                message.send_async(&mut socket).await.unwrap();
            }
            SubmitTemplate(block) => {
                println!("received allegedly mined template");
                let mut blockchain =
                    crate::BLOCKCHAIN.write().await;
                if let Err(e) =
                    blockchain.add_block(block.clone())
                {
                    println!(
                        "block rejected: {e}, closing connection"
                    );
                    return;
                }

                blockchain.rebuild_utxos();

                println!("block looks good, broadcasting");

                // send block to all friend nodes
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();

                for node in nodes {
                    if let Some(mut stream) =
                        crate::NODES.get_mut(&node)
                    {
                        let message =
                            Message::NewBlock(block.clone());
                        if message
                            .send_async(&mut *stream)
                            .await
                            .is_err()
                        {
                            println!(
                                "failed to send block to {}",
                                node
                            );
                        }
                    }
                }
            }
            SubmitTransaction(tx) => {
                println!("submmit tx");
                let mut blockchain =
                    crate::BLOCKCHAIN.write().await;
                if let Err(e) =
                    blockchain.add_to_mempool(tx.clone())
                {
                    println!("transaction rejected, closing connection: {e}");
                    return;
                }

                println!("added transaction to mempool");

                // send transaction to all friend nodes
                let nodes = crate::NODES
                    .iter()
                    .map(|x| x.key().clone())
                    .collect::<Vec<_>>();

                for node in nodes {
                    println!("sending to friend: {node}");
                    if let Some(mut stream) =
                        crate::NODES.get_mut(&node)
                    {
                        let message =
                            Message::NewTransaction(tx.clone());
                        if message
                            .send_async(&mut *stream)
                            .await
                            .is_err()
                        {
                            println!("failed to send transaction to {}", node);
                        }
                    }
                }

                println!("transaction sent to friends");
            }
            FetchTemplate(pubkey) => {
                let blockchain = crate::BLOCKCHAIN.read().await;

                let mut transactions = vec![];
                // insert transactions from mempool
                transactions.extend(
                    blockchain
                        .mempool()
                        .iter()
                        .take(btclib::BLOCK_TRANSACTION_CAP)
                        .map(|(_, tx)| tx)
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                // insert coinbase tx with pubkey
                transactions.insert(
                    0,
                    Transaction {
                        inputs: vec![],
                        outputs: vec![TransactionOutput {
                            pubkey,
                            unique_id: Uuid::new_v4(),
                            value: 0,
                        }],
                    },
                );

                let merkle_root =
                    MerkleRoot::calculate(&transactions);

                let mut block = Block::new(
                    BlockHeader {
                        timestamp: Utc::now(),
                        prev_block_hash: blockchain
                            .blocks()
                            .last()
                            .map(|last_block| last_block.hash())
                            .unwrap_or(Hash::zero()),
                        nonce: 0,
                        target: blockchain.target(),
                        merkle_root,
                    },
                    transactions,
                );

                let miner_fees = match block
                    .calculate_miner_fees(blockchain.utxos())
                {
                    Ok(fees) => fees,
                    Err(e) => {
                        eprintln!("{e}");
                        return;
                    }
                };

                let reward = blockchain.calculate_block_reward();

                // update coinbase tx with reward
                block.transactions[0].outputs[0].value =
                    reward + miner_fees;

                // recalculate merkle root
                block.header.merkle_root =
                    MerkleRoot::calculate(&block.transactions);

                let message = Template(block);
                message.send_async(&mut socket).await.unwrap();
            }
        }
    }
}
