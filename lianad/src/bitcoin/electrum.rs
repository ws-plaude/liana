//! Electrum backend, built on a bwk scanner.
//!
//! bwk keeps its own transaction, address and header stores under lianad's data
//! directory. lianad's SQLite database stays authoritative for the wallet; the
//! bwk scanner is only our view of the chain. Unlike the previous backend, block
//! headers are validated and transaction inclusion is checked against a merkle
//! proof, so a server cannot make us believe in a block that does not exist.
//!
//! lianad signs its own transactions, so it takes the scanner alone rather than
//! `bwk::Account`: no signers, no mnemonic, no wallet-side transaction builder.

use std::{
    collections::BTreeMap,
    convert::TryFrom,
    fmt,
    path::PathBuf,
    sync::{mpsc, Arc},
};

use bwk_coin::{Coin, CoinSpendInfo, CoinStatus, KeyChain};
use bwk_descriptor::derivator::{self, SpkDerivator};
use bwk_electrum::{
    client,
    coin_store::CoinEntry,
    header_store::{HeaderStore, StartError},
    notification::{Notification, OpenError, TxListenerNotif},
    parse_electrum_url,
    tx_store::TxEntry,
    ElectrumScanner, ElectrumScheme, ScannerConfig,
};
use bwk_persist::PersistenceKind;
use liana::descriptors::LianaDescriptor;
use miniscript::bitcoin::{self, bip32::ChildNumber, block::Header};

use crate::{
    bitcoin::{Block, BlockChainTip, SpentCoin, UTxO, UTxOAddress, COINBASE_MATURITY},
    config,
};

// Directory holding the bwk stores, under lianad's data directory.
const BWK_DIR: &str = "bwk";

// Name of the single bwk account we open. Liana is a single-wallet daemon.
const BWK_ACCOUNT: &str = "lianad";

// How many addresses past the last one we know is used to keep watching. Each
// address is a separate subscription, so we don't want to overload the server.
const LOOK_AHEAD: u32 = 30;

// How many recent block hashes to remember so we can locate the fork point
// after a reorg. The header store only keeps the active chain, so the previous
// one has to be remembered here or the fork is not findable. One retarget
// interval, which is also the header store's backfill granularity.
const REORG_WINDOW: u32 = 2016;

/// An error in the Electrum interface.
#[derive(Debug)]
pub enum ElectrumError {
    Address(String),
    DomainValidation,
    Descriptor(derivator::Error),
    HeaderStore(StartError),
    Scanner(OpenError),
    MissingReceiver,
}

impl fmt::Display for ElectrumError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ElectrumError::Address(e) => write!(f, "Invalid Electrum server address: '{e}'."),
            ElectrumError::DomainValidation => write!(
                f,
                "Disabling SSL domain validation is not supported by the Electrum backend."
            ),
            ElectrumError::Descriptor(e) => {
                write!(f, "Descriptor not usable with the Electrum backend: '{e}'.")
            }
            ElectrumError::HeaderStore(e) => write!(f, "Error starting the header store: '{e}'."),
            ElectrumError::Scanner(e) => write!(f, "Error opening the wallet stores: '{e}'."),
            ElectrumError::MissingReceiver => {
                write!(f, "The wallet notification channel was already taken.")
            }
        }
    }
}

/// The spending transaction of one of our coins.
#[derive(Clone, Copy)]
struct Spender {
    txid: bitcoin::Txid,
    height: Option<i32>,
}

/// Interface for the Electrum backend.
pub struct Electrum {
    scanner: ElectrumScanner,
    header_store: Arc<HeaderStore>,
    notifications: mpsc::Receiver<Notification>,
    /// The Electrum endpoint, in the form bwk's client expects.
    url: String,
    port: u16,
    /// The genesis block, computed from the network. Also the tip we report
    /// while the header store is still empty: we know of no block beyond it.
    genesis: BlockChainTip,
    genesis_time: u32,
    /// Block hashes we last read from the header store, to locate the fork
    /// point after a reorg.
    observed: BTreeMap<u32, bitcoin::BlockHash>,
    /// The tip we reported to the poller at the previous sync.
    last_tip: Option<BlockChainTip>,
    /// Whether the listener has connected at least once. Until it has, the
    /// stores say nothing about what is still on chain.
    synced: bool,
    /// Set when the listener reported it stopped; the next sync restarts it.
    restart: bool,
}

impl Electrum {
    pub fn new(
        electrum_config: &config::ElectrumConfig,
        main_descriptor: &LianaDescriptor,
        network: bitcoin::Network,
        data_dir: PathBuf,
    ) -> Result<Self, ElectrumError> {
        // bwk always validates the server certificate against the domain. Refuse
        // rather than silently ignore the setting.
        if !electrum_config.validate_domain {
            return Err(ElectrumError::DomainValidation);
        }
        let (url, port) = endpoint(&electrum_config.addr)?;

        let descriptor = main_descriptor.descriptor().clone();
        // bwk panics on a descriptor its deriver rejects, so check it up front.
        SpkDerivator::new(descriptor.clone(), network).map_err(ElectrumError::Descriptor)?;

        let config = ScannerConfig {
            data_dir,
            dir_name: BWK_DIR.to_string(),
            account: BWK_ACCOUNT.to_string(),
            electrum_url: Some(url.clone()),
            electrum_port: Some(port),
            offline: None,
            network,
            look_ahead: LOOK_AHEAD,
            descriptor,
            persist: true,
            // lianad stores the labels in its own database.
            skip_labels: true,
            persist_kind: PersistenceKind::Json,
        };

        // Anchor the header chain at genesis. lianad's own tip starts there, and
        // a rescan may ask for a block older than any recent window.
        let header_store = HeaderStore::start(
            url.clone(),
            port,
            network,
            Some(config.headers_path()),
            Some(0),
        )
        .map_err(ElectrumError::HeaderStore)?;

        let mut scanner = ElectrumScanner::try_new_with_header_store(config, header_store.clone())
            .map_err(ElectrumError::Scanner)?;
        let notifications = scanner.receiver().ok_or(ElectrumError::MissingReceiver)?;
        scanner.start_electrum();

        let genesis = bitcoin::constants::genesis_block(network);
        Ok(Self {
            scanner,
            header_store,
            notifications,
            url,
            port,
            genesis: BlockChainTip {
                hash: genesis.block_hash(),
                height: 0,
            },
            genesis_time: genesis.header.time,
            observed: BTreeMap::new(),
            last_tip: None,
            synced: false,
            restart: false,
        })
    }

    pub fn genesis_block(&self) -> BlockChainTip {
        self.genesis
    }

    pub fn genesis_block_timestamp(&self) -> u32 {
        self.genesis_time
    }

    /// The tip of the validated header chain, or the genesis block if we have
    /// not synced any header yet.
    pub fn chain_tip(&self) -> BlockChainTip {
        self.header_store
            .tip_with_hash()
            .and_then(|(height, hash)| block_chain_tip(height, hash))
            .unwrap_or(self.genesis)
    }

    pub fn tip_time(&self) -> Option<u32> {
        let tip = self.header_store.tip()?;
        self.header_store.header(tip).map(|header| header.time)
    }

    pub fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        is_in_chain(|height| self.header_store.block_hash(height), tip)
    }

    /// A reorg we did not witness, for instance one that happened while the
    /// daemon was down. The header store only keeps the active chain, so there
    /// is no way to tell where the caller's chain forked away from it: start
    /// over from the genesis block. That is cheap here, as the poller rebuilds
    /// its coin state from the wallet stores rather than from the network.
    pub fn common_ancestor(&self, _tip: &BlockChainTip) -> Option<BlockChainTip> {
        log::warn!(
            "Block chain reorganization we did not witness. Starting over from the genesis block."
        );
        Some(self.genesis)
    }

    pub fn block_before_date(&self, timestamp: u32) -> Option<BlockChainTip> {
        let tip = self.header_store.tip()?;
        block_before_date(|h| self.header_store.header(h), tip, timestamp)
    }

    /// Reconcile the event-driven bwk scanner with the poller: apply whatever
    /// the background listener told us, widen the watched address window to the
    /// indices lianad has handed out, and report a reorg if the chain we last
    /// saw is no longer the one the header store holds.
    pub fn sync_wallet(
        &mut self,
        receive_index: ChildNumber,
        change_index: ChildNumber,
    ) -> Result<Option<BlockChainTip>, ElectrumError> {
        self.drain_notifications();
        if self.restart {
            log::info!("Restarting the Electrum listener.");
            self.scanner.restart_electrum();
            self.restart = false;
        }
        self.widen_watch_window(receive_index, change_index);

        let tip = self.chain_tip();
        let ancestor = self
            .last_tip
            .filter(|_| self.chain_changed())
            .map(|last| self.rollback_target(last.height));
        self.record_chain(tip);
        self.last_tip = Some(tip);
        Ok(ancestor)
    }

    /// Get every coin the wallet knows about. The poller discards the ones it
    /// already has. We don't filter on the tip the poller passes: bwk may learn
    /// about an old coin at any time, for instance when a newly watched address
    /// turns out to have a history, and such a coin would then be lost forever.
    pub fn received_coins(&self) -> Vec<UTxO> {
        let txs = self.transactions();
        let tip_height = self.chain_tip().height;
        self.scanner
            .coins()
            .values()
            .filter_map(|entry| utxo(entry, &txs, tip_height))
            .collect()
    }

    pub fn confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        let coins = self.scanner.coins();
        let txs = self.transactions();
        let tip_height = self.chain_tip().height;
        let mut confirmed = Vec::new();
        let mut expired = Vec::new();

        for op in outpoints {
            let entry = match coins.get(op) {
                Some(entry) => entry,
                None => {
                    // bwk drops a transaction as soon as it leaves the history
                    // the server reports for our addresses, so an unknown coin
                    // means its deposit is gone. Before the first connection the
                    // stores are just not populated yet.
                    if self.synced {
                        expired.push(*op);
                    }
                    continue;
                }
            };
            let height = match confirmation_height(&entry.coin).and_then(block_height) {
                Some(height) => height,
                None => continue,
            };
            if is_immature(entry, &txs, tip_height) {
                log::debug!(
                    "Coin at '{op}' comes from an immature coinbase transaction at block height \
                     {height}. Not marking it as confirmed for now."
                );
                continue;
            }
            match self.block_time(height) {
                Some(time) => confirmed.push((*op, height, time)),
                // The header chain has not been backfilled down to this block
                // yet. Confirm the coin at the next poll rather than make up a
                // block time.
                None => log::debug!(
                    "No validated header at height {height} yet. \
                     Not marking coin '{op}' as confirmed for now."
                ),
            }
        }

        (confirmed, expired)
    }

    pub fn spending_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        let coins = self.scanner.coins();
        let spenders = self.spenders();
        outpoints
            .iter()
            .filter(|op| coins.contains_key(op))
            .filter_map(|op| spenders.get(op).map(|spender| (*op, spender.txid)))
            .collect()
    }

    pub fn spent_coins(
        &self,
        outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (Vec<SpentCoin>, Vec<bitcoin::OutPoint>) {
        let coins = self.scanner.coins();
        let spenders = self.spenders();
        let mut spent = Vec::new();
        let mut expired_spending = Vec::new();

        for (op, spend_txid) in outpoints {
            if !coins.contains_key(op) {
                continue;
            }
            let spender = match spenders.get(op) {
                Some(spender) => spender,
                None => {
                    expired_spending.push(*op);
                    continue;
                }
            };
            // The spend we were told about got replaced. Report it as expired so
            // the poller unspends the coin, then picks the new spender up.
            if spender.txid != *spend_txid {
                expired_spending.push(*op);
            }
            if let Some(height) = spender.height {
                match self.block_time(height) {
                    // Report the transaction that actually spends the coin, not
                    // the one we were asked about, which may have been replaced.
                    Some(time) => spent.push((*op, spender.txid, height, time)),
                    None => log::debug!(
                        "No validated header at height {height} yet. \
                         Not marking coin '{op}' as spent for now."
                    ),
                }
            }
        }

        (spent, expired_spending)
    }

    pub fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        let entry = self
            .scanner
            .tx_history()
            .into_iter()
            .find(|entry| entry.txid() == *txid)?;
        let block = entry
            .height()
            .and_then(block_height)
            .zip(entry.block_hash())
            .and_then(|(height, hash)| {
                Some(Block {
                    hash,
                    height,
                    time: self.block_time(height)?,
                })
            });
        Some((entry.tx().clone(), block))
    }

    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), String> {
        let mut client = client::Client::new(&self.url, self.port).map_err(|e| e.to_string())?;
        client.broadcast(tx).map_err(|e| e.to_string())?;
        // Show the spend right away instead of waiting for the server to tell us
        // about a transaction we just sent it ourselves.
        self.scanner.record_unconfirmed_spend(tx);
        Ok(())
    }

    /// Every wallet transaction, by txid.
    fn transactions(&self) -> BTreeMap<bitcoin::Txid, TxEntry> {
        self.scanner
            .tx_history()
            .into_iter()
            .map(|entry| (entry.txid(), entry))
            .collect()
    }

    /// The transaction spending each of our coins, if any.
    fn spenders(&self) -> BTreeMap<bitcoin::OutPoint, Spender> {
        let mut spenders: BTreeMap<bitcoin::OutPoint, Spender> = BTreeMap::new();
        for entry in self.scanner.tx_history() {
            let spender = Spender {
                txid: entry.txid(),
                height: entry.height().and_then(block_height),
            };
            for txin in &entry.tx().input {
                // Two transactions spending the same coin can only both be in the
                // store while a replacement has not been dropped yet. Prefer the
                // mined one.
                let known = spenders.get(&txin.previous_output);
                if known.is_none_or(|k| k.height.is_none() && spender.height.is_some()) {
                    spenders.insert(txin.previous_output, spender);
                }
            }
        }
        spenders
    }

    /// The timestamp of the block at this height in the validated chain.
    fn block_time(&self, height: i32) -> Option<u32> {
        let height = u32::try_from(height).ok()?;
        self.header_store.header(height).map(|header| header.time)
    }

    /// Whether any block we recorded is no longer part of the header chain.
    fn chain_changed(&self) -> bool {
        chain_changed(&self.observed, |height| {
            self.header_store.block_hash(height)
        })
    }

    /// Where the poller must roll its state back to after a reorg we witnessed:
    /// the highest block at or below `height` that both the chain we recorded
    /// and the header chain still agree on. Must be called before the record is
    /// refreshed, which is what still holds the chain that was replaced.
    fn rollback_target(&self, height: i32) -> BlockChainTip {
        fork_point(&self.observed, |h| self.header_store.block_hash(h), height).unwrap_or_else(
            || {
                log::error!(
                    "Block chain reorganization deeper than the last {REORG_WINDOW} blocks. \
                     Starting over from the genesis block."
                );
                self.genesis
            },
        )
    }

    /// Take a fresh record of the tail of the header chain.
    fn record_chain(&mut self, tip: BlockChainTip) {
        let tip_height = match u32::try_from(tip.height) {
            Ok(height) => height,
            Err(_) => return,
        };
        self.observed.clear();
        for height in tip_height.saturating_sub(REORG_WINDOW)..=tip_height {
            if let Some(hash) = self.header_store.block_hash(height) {
                self.observed.insert(height, hash);
            }
        }
    }

    /// Make sure bwk watches at least up to the derivation indices lianad has
    /// handed out. Its receive tip only moves through `new_addr`; its change tip
    /// only moves when a change coin lands, so change relies on the look-ahead.
    fn widen_watch_window(&mut self, receive_index: ChildNumber, change_index: ChildNumber) {
        let receive_index: u32 = receive_index.into();
        while self.scanner.recv_watch_tip() < receive_index {
            self.scanner.new_addr();
        }
        let change_index: u32 = change_index.into();
        let change_watch_tip = self.scanner.change_watch_tip();
        if change_watch_tip < change_index {
            log::warn!(
                "Change addresses are watched up to index {change_watch_tip} but index \
                 {change_index} has been handed out. Coins on the addresses in between would not \
                 be seen."
            );
        }
    }

    /// Apply what the background listener reported since the previous sync.
    fn drain_notifications(&mut self) {
        while let Ok(notification) = self.notifications.try_recv() {
            match notification {
                Notification::Electrum(TxListenerNotif::Connected(server)) => {
                    log::info!("Connected to the Electrum server at '{server}'.");
                    self.synced = true;
                }
                Notification::Electrum(TxListenerNotif::Stopped) | Notification::Stopped => {
                    log::warn!("The Electrum listener stopped.");
                    self.restart = true;
                }
                Notification::Electrum(TxListenerNotif::Error(e)) => {
                    log::error!("Electrum listener error: '{e}'.")
                }
                // Verification is a background signal: a coin keeps the state the
                // server reported for it, we only record that the proof failed.
                Notification::ValidationFailed(failure) => {
                    log::error!("Chain validation failed: '{failure:?}'.")
                }
                Notification::Error(e) => log::error!("Wallet error: '{e:?}'."),
                Notification::InvalidElectrumConfig => {
                    log::error!("Invalid Electrum server configuration.")
                }
                Notification::InvalidLookAhead => log::error!("Invalid address look-ahead."),
                Notification::Electrum(TxListenerNotif::Started)
                | Notification::AddressTipChanged
                | Notification::CoinUpdate
                | Notification::HeaderStoreUpdated => {}
            }
        }
    }
}

/// Split a configured Electrum address into the URL and port bwk's client
/// expects. The client reads the scheme back from the URL, so keep the prefix.
fn endpoint(addr: &str) -> Result<(String, u16), ElectrumError> {
    let (host, port, scheme) = parse_electrum_url(addr).map_err(ElectrumError::Address)?;
    let host = host.ok_or_else(|| ElectrumError::Address(format!("no host in '{addr}'")))?;
    let port = port.ok_or_else(|| ElectrumError::Address(format!("no port in '{addr}'")))?;
    let url = match scheme {
        ElectrumScheme::Ssl => format!("ssl://{host}"),
        ElectrumScheme::Tcp => host,
    };
    Ok((url, port))
}

/// Whether this block is the one the chain holds at its height.
fn is_in_chain(
    block_hash: impl Fn(u32) -> Option<bitcoin::BlockHash>,
    tip: &BlockChainTip,
) -> bool {
    u32::try_from(tip.height)
        .ok()
        .and_then(block_hash)
        .map(|hash| hash == tip.hash)
        .unwrap_or(false)
}

/// Whether any of the blocks we recorded is no longer part of the chain.
fn chain_changed(
    observed: &BTreeMap<u32, bitcoin::BlockHash>,
    block_hash: impl Fn(u32) -> Option<bitcoin::BlockHash>,
) -> bool {
    observed
        .iter()
        .any(|(height, hash)| block_hash(*height) != Some(*hash))
}

/// The highest block at or below `height` that both the chain we recorded and
/// the chain we hold now agree on. `None` when they fork below what we
/// recorded, which leaves nothing to anchor on.
fn fork_point(
    observed: &BTreeMap<u32, bitcoin::BlockHash>,
    block_hash: impl Fn(u32) -> Option<bitcoin::BlockHash>,
    height: i32,
) -> Option<BlockChainTip> {
    let height = u32::try_from(height).ok()?;
    observed
        .range(..=height)
        .rev()
        .find(|(h, hash)| block_hash(**h) == Some(**hash))
        .and_then(|(height, hash)| block_chain_tip(*height, *hash))
}

/// Block heights above `i32::MAX` cannot happen on any real chain. Refuse one
/// rather than wrap it into a negative height.
fn block_height(height: u64) -> Option<i32> {
    i32::try_from(height).ok()
}

fn block_chain_tip(height: u32, hash: bitcoin::BlockHash) -> Option<BlockChainTip> {
    i32::try_from(height)
        .ok()
        .map(|height| BlockChainTip { hash, height })
}

/// The height lianad should consider a coin confirmed at.
///
/// `ConfirmedUnverified` counts as confirmed just like `Confirmed`: the server
/// reported the coin's transaction as mined and its block header is known, only
/// the merkle proof is still pending. A coin being spent overwrites the status,
/// so for those the recorded height is what says whether it was mined.
fn confirmation_height(coin: &Coin) -> Option<u64> {
    match coin.status {
        CoinStatus::Unconfirmed => None,
        CoinStatus::ConfirmedUnverified
        | CoinStatus::Confirmed
        | CoinStatus::BeingSpend
        | CoinStatus::Spent => coin.height,
    }
}

/// Whether this coin comes from a coinbase transaction that has not matured.
/// bwk has no notion of coinbase maturity, so derive it from the transaction.
fn is_immature(entry: &CoinEntry, txs: &BTreeMap<bitcoin::Txid, TxEntry>, tip_height: i32) -> bool {
    let is_coinbase = txs
        .get(&entry.coin.outpoint.txid)
        .map(|tx| tx.tx().is_coinbase())
        .unwrap_or(false);
    if !is_coinbase {
        return false;
    }
    confirmation_height(&entry.coin)
        .and_then(block_height)
        .and_then(|height| tip_height.checked_sub(height))
        .map(|confirmations| confirmations < COINBASE_MATURITY)
        .unwrap_or(true)
}

/// Turn a bwk coin into the unspent output lianad tracks. Returns `None` for a
/// coin we cannot place on one of our two keychains, which should not happen
/// for a descriptor account.
fn utxo(
    entry: &CoinEntry,
    txs: &BTreeMap<bitcoin::Txid, TxEntry>,
    tip_height: i32,
) -> Option<UTxO> {
    let (keychain, index) = match &entry.coin.spend_info {
        CoinSpendInfo::Bip32 { coin_path, .. } => *coin_path,
        CoinSpendInfo::Sp { .. } => {
            log::error!(
                "Silent payment coin at '{}' in a descriptor wallet.",
                entry.coin.outpoint
            );
            return None;
        }
    };
    let is_change = match keychain {
        KeyChain::Receive => false,
        KeyChain::Change => true,
        KeyChain::Custom(_) => {
            log::error!("Coin at '{}' on an unknown keychain.", entry.coin.outpoint);
            return None;
        }
    };
    let derivation_index = ChildNumber::from_normal_idx(index).ok()?;
    Some(UTxO {
        outpoint: entry.coin.outpoint,
        amount: entry.coin.txout.value,
        block_height: confirmation_height(&entry.coin).and_then(block_height),
        address: UTxOAddress::DerivIndex(derivation_index, is_change),
        is_immature: is_immature(entry, txs, tip_height),
    })
}

/// The last block of the chain with a timestamp below `target`, by binary
/// search over the header chain.
///
/// Block timestamps are not strictly increasing, so a block a little below the
/// result may still have a timestamp above the target.
fn block_before_date(
    header: impl Fn(u32) -> Option<Header>,
    tip_height: u32,
    target: u32,
) -> Option<BlockChainTip> {
    let genesis_time = header(0)?.time;
    let tip_time = header(tip_height)?.time;
    if !(genesis_time..tip_time).contains(&target) {
        return None;
    }

    let mut start = 0;
    let mut end = tip_height;
    while start < end {
        let current = start + (end - start) / 2;
        // We want the last block with a timestamp below the target, not the
        // first one with a higher one.
        let next = current + 1;
        if target > header(next)?.time {
            start = next;
        } else {
            end = current;
        }
    }

    block_chain_tip(start, header(start)?.block_hash())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use miniscript::{
        bitcoin::{
            absolute, hashes::Hash, transaction, Amount, CompactTarget, OutPoint, ScriptBuf,
            Sequence, Transaction, TxIn, TxMerkleNode, TxOut, Txid, Witness,
        },
        Descriptor, DescriptorPublicKey,
    };

    const DESC: &str = "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*),older(10000))))";

    // A chain of headers linked by their previous block hash, so that the block
    // hashes differ from one height to the next. Timestamps are ten minutes
    // apart, starting at the given time.
    fn chain(len: u32, first_time: u32) -> Vec<Header> {
        let mut headers = Vec::with_capacity(len as usize);
        let mut prev_blockhash = bitcoin::BlockHash::all_zeros();
        for i in 0..len {
            let header = Header {
                version: bitcoin::block::Version::ONE,
                prev_blockhash,
                merkle_root: TxMerkleNode::all_zeros(),
                time: first_time + i * 600,
                bits: CompactTarget::from_consensus(0x207fffff),
                nonce: i,
            };
            prev_blockhash = header.block_hash();
            headers.push(header);
        }
        headers
    }

    fn coin(
        status: CoinStatus,
        height: Option<u64>,
        txid: Txid,
        index: u32,
        change: bool,
    ) -> CoinEntry {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(DESC).unwrap();
        CoinEntry {
            coin: Coin {
                txout: TxOut {
                    value: Amount::from_sat(100_000),
                    script_pubkey: ScriptBuf::new(),
                },
                outpoint: OutPoint { txid, vout: 0 },
                height,
                sequence: Sequence::ZERO,
                status,
                label: None,
                satisfaction_size: 100,
                spend_info: CoinSpendInfo::Bip32 {
                    coin_path: (
                        if change {
                            KeyChain::Change
                        } else {
                            KeyChain::Receive
                        },
                        index,
                    ),
                    descriptor,
                    secret_key: None,
                },
            },
            address: bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
                .unwrap(),
        }
    }

    fn tx(coinbase: bool) -> Transaction {
        let previous_output = if coinbase {
            OutPoint::null()
        } else {
            OutPoint {
                txid: Txid::from_byte_array([1; 32]),
                vout: 0,
            }
        };
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ZERO,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[test]
    fn confirmed_and_confirmed_unverified_are_both_confirmed() {
        let txid = tx(false).compute_txid();
        for status in [CoinStatus::Confirmed, CoinStatus::ConfirmedUnverified] {
            let entry = coin(status, Some(42), txid, 0, false);
            assert_eq!(confirmation_height(&entry.coin), Some(42));
        }

        let entry = coin(CoinStatus::Unconfirmed, None, txid, 0, false);
        assert_eq!(confirmation_height(&entry.coin), None);

        // A spent coin keeps the height of its own deposit transaction.
        let entry = coin(CoinStatus::Spent, Some(7), txid, 0, false);
        assert_eq!(confirmation_height(&entry.coin), Some(7));
        let entry = coin(CoinStatus::BeingSpend, Some(7), txid, 0, false);
        assert_eq!(confirmation_height(&entry.coin), Some(7));
        let entry = coin(CoinStatus::Spent, None, txid, 0, false);
        assert_eq!(confirmation_height(&entry.coin), None);
    }

    #[test]
    fn coin_maps_to_utxo() {
        let deposit = tx(false);
        let txid = deposit.compute_txid();
        let txs = BTreeMap::new();

        let entry = coin(CoinStatus::Confirmed, Some(120), txid, 5, true);
        let change = utxo(&entry, &txs, 200).unwrap();
        assert_eq!(change.outpoint, OutPoint { txid, vout: 0 });
        assert_eq!(change.amount, Amount::from_sat(100_000));
        assert_eq!(change.block_height, Some(120));
        assert!(!change.is_immature);
        match change.address {
            UTxOAddress::DerivIndex(index, is_change) => {
                assert_eq!(index, ChildNumber::from_normal_idx(5).unwrap());
                assert!(is_change);
            }
            UTxOAddress::Address(_) => panic!("must be a derivation index"),
        }

        let entry = coin(CoinStatus::Unconfirmed, None, txid, 0, false);
        let deposit = utxo(&entry, &txs, 200).unwrap();
        assert_eq!(deposit.block_height, None);
        match deposit.address {
            UTxOAddress::DerivIndex(index, is_change) => {
                assert_eq!(index, ChildNumber::from_normal_idx(0).unwrap());
                assert!(!is_change);
            }
            UTxOAddress::Address(_) => panic!("must be a derivation index"),
        }
    }

    #[test]
    fn coinbase_is_immature_until_it_matures() {
        let coinbase = tx(true);
        let txid = coinbase.compute_txid();
        let mut txs = BTreeMap::new();
        txs.insert(txid, TxEntry::unconfirmed(coinbase));

        // One short of the maturity.
        let entry = coin(CoinStatus::Confirmed, Some(100), txid, 0, false);
        assert!(is_immature(&entry, &txs, 199));
        assert!(utxo(&entry, &txs, 199).unwrap().is_immature);
        // Exactly at the maturity.
        assert!(!is_immature(&entry, &txs, 200));
        assert!(!utxo(&entry, &txs, 200).unwrap().is_immature);
        // Not mined yet.
        let entry = coin(CoinStatus::Unconfirmed, None, txid, 0, false);
        assert!(is_immature(&entry, &txs, 200));

        // A regular deposit never is.
        let deposit = tx(false);
        let txid = deposit.compute_txid();
        let mut txs = BTreeMap::new();
        txs.insert(txid, TxEntry::unconfirmed(deposit));
        let entry = coin(CoinStatus::Confirmed, Some(100), txid, 0, false);
        assert!(!is_immature(&entry, &txs, 100));
    }

    // Hashes of a header chain, by height, as the header store exposes them.
    fn hashes(headers: &[Header]) -> impl Fn(u32) -> Option<bitcoin::BlockHash> + '_ {
        move |height: u32| {
            headers
                .get(height as usize)
                .map(|header| header.block_hash())
        }
    }

    fn recorded(headers: &[Header]) -> BTreeMap<u32, bitcoin::BlockHash> {
        headers
            .iter()
            .enumerate()
            .map(|(i, header)| (i as u32, header.block_hash()))
            .collect()
    }

    // A chain sharing the first `common + 1` blocks with `headers`, then
    // diverging up to the same length.
    fn fork(headers: &[Header], common: usize) -> Vec<Header> {
        let mut forked = headers[..=common].to_vec();
        for i in common + 1..headers.len() {
            let header = Header {
                prev_blockhash: forked[i - 1].block_hash(),
                nonce: 1_000 + i as u32,
                ..headers[i]
            };
            forked.push(header);
        }
        forked
    }

    #[test]
    fn tip_is_in_chain_only_at_its_own_height_and_hash() {
        let headers = chain(10, 1_600_000_000);

        assert!(is_in_chain(
            hashes(&headers),
            &BlockChainTip {
                hash: headers[4].block_hash(),
                height: 4
            }
        ));
        // Same hash, wrong height.
        assert!(!is_in_chain(
            hashes(&headers),
            &BlockChainTip {
                hash: headers[4].block_hash(),
                height: 5
            }
        ));
        // Right height, hash from another chain.
        assert!(!is_in_chain(
            hashes(&headers),
            &BlockChainTip {
                hash: chain(10, 1_700_000_000)[4].block_hash(),
                height: 4
            }
        ));
        // Above the chain.
        assert!(!is_in_chain(
            hashes(&headers),
            &BlockChainTip {
                hash: headers[9].block_hash(),
                height: 42
            }
        ));
    }

    #[test]
    fn fork_point_is_the_highest_block_both_chains_share() {
        let headers = chain(10, 1_600_000_000);
        let observed = recorded(&headers);

        // The chain did not move: nothing was reorged and the fork point is the
        // block we are asked about.
        assert!(!chain_changed(&observed, hashes(&headers)));
        let ancestor = fork_point(&observed, hashes(&headers), 9).unwrap();
        assert_eq!(ancestor.height, 9);

        // A chain forking away after height 5.
        let forked = fork(&headers, 5);
        assert!(chain_changed(&observed, hashes(&forked)));
        let ancestor = fork_point(&observed, hashes(&forked), 9).unwrap();
        assert_eq!(ancestor.height, 5);
        assert_eq!(ancestor.hash, headers[5].block_hash());

        // Asked below the fork, the answer is the block itself.
        let ancestor = fork_point(&observed, hashes(&forked), 3).unwrap();
        assert_eq!(ancestor.height, 3);
        assert_eq!(ancestor.hash, headers[3].block_hash());

        // A chain sharing no block with the one we recorded.
        let other = chain(10, 1_700_000_000);
        assert!(chain_changed(&observed, hashes(&other)));
        assert!(fork_point(&observed, hashes(&other), 9).is_none());
    }

    #[test]
    fn block_before_date_finds_the_last_block_below_the_target() {
        // Heights 0..=9, ten minutes apart from 1_600_000_000.
        let headers = chain(10, 1_600_000_000);
        let header = |h: u32| headers.get(h as usize).copied();

        // Halfway between block 3 and block 4.
        let target = 1_600_000_000 + 3 * 600 + 300;
        let block = block_before_date(header, 9, target).unwrap();
        assert_eq!(block.height, 3);
        assert_eq!(block.hash, headers[3].block_hash());

        // Exactly on a block timestamp: that block is not below the target.
        let block = block_before_date(header, 9, 1_600_000_000 + 5 * 600).unwrap();
        assert_eq!(block.height, 4);

        // Just after genesis.
        let block = block_before_date(header, 9, 1_600_000_000 + 1).unwrap();
        assert_eq!(block.height, 0);

        // Outside the chain's range.
        assert!(block_before_date(header, 9, 1_500_000_000).is_none());
        assert!(block_before_date(header, 9, 1_700_000_000).is_none());
        // The tip's own timestamp is not below itself.
        assert!(block_before_date(header, 9, 1_600_000_000 + 9 * 600).is_none());
    }
}
