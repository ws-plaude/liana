use std::{collections::HashSet, env, process, str::FromStr};

use lianad::{config::ElectrumConfig, miniscript::bitcoin::Txid, ElectrumClient};

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let addr = args.next().ok_or_else(|| {
        "usage: listspendtxs_electrum <electrum-addr> [conflicting-spender-txid]...".to_string()
    })?;
    let txids = args
        .map(|arg| Txid::from_str(&arg).map_err(|error| error.to_string()))
        .collect::<Result<HashSet<_>, _>>()?;
    if txids.is_empty() {
        println!("Found 0 mempool entries.");
        return Ok(());
    }

    let client = ElectrumClient::new(&ElectrumConfig {
        addr,
        validate_domain: true,
    })
    .map_err(|error| error.to_string())?;
    let entries = client
        .measure_mempool_entries(txids)
        .map_err(|error| error.to_string())?;

    println!("Found {} mempool entries.", entries.len());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
