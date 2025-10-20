use crate::lightning::ln_storage::{
    LightningStorage, NetworkGraph, NodesAddressesMap, NodesAddressesMapShared, Scorer, TrustedNodesShared,
};
use async_trait::async_trait;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::{BlockHash, Network, Txid};
use bitcoin_hashes::hex::FromHex;
use common::async_blocking;
use common::log::LogState;
use lightning::chain::channelmonitor::ChannelMonitor;
use lightning::chain::keysinterface::{KeysInterface, Sign};
use lightning::routing::scoring::{ProbabilisticScorer, ProbabilisticScoringParameters};
use lightning::util::persist::KVStorePersister;
use lightning::util::ser::{ReadableArgs, Writeable};
use mm2_io::fs::{check_dir_operations, invalid_data_err, read_json, write_json};
use secp256k1v24::PublicKey;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, Cursor};
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

#[cfg(target_family = "unix")]
use std::os::unix::io::AsRawFd;

#[cfg(target_family = "windows")]
use {std::ffi::OsStr, std::os::windows::ffi::OsStrExt};

const USE_TMP_FILE: bool = true;

pub struct LightningFilesystemPersister {
    main_path: PathBuf,
    backup_path: Option<PathBuf>,
}

impl LightningFilesystemPersister {
    /// Initialize a new LightningPersister and set the path to the individual channels'
    /// files.
    #[inline]
    pub fn new(main_path: PathBuf, backup_path: Option<PathBuf>) -> Self {
        Self { main_path, backup_path }
    }

    /// Get the directory which was provided when this persister was initialized.
    #[inline]
    pub fn main_path(&self) -> PathBuf {
        self.main_path.clone()
    }

    /// Get the backup directory which was provided when this persister was initialized.
    #[inline]
    pub fn backup_path(&self) -> Option<PathBuf> {
        self.backup_path.clone()
    }

    pub fn nodes_addresses_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("channel_nodes_data");
        path
    }

    pub fn nodes_addresses_backup_path(&self) -> Option<PathBuf> {
        self.backup_path().map(|mut backup_path| {
            backup_path.push("channel_nodes_data");
            backup_path
        })
    }

    pub fn network_graph_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("network_graph");
        path
    }

    pub fn scorer_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("scorer");
        path
    }

    pub fn trusted_nodes_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("trusted_nodes");
        path
    }

    pub fn manager_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("manager");
        path
    }

    pub fn monitors_path(&self) -> PathBuf {
        let mut path = self.main_path();
        path.push("monitors");
        path
    }

    pub fn monitors_backup_path(&self) -> Option<PathBuf> {
        self.backup_path().map(|mut backup_path| {
            backup_path.push("monitors");
            backup_path
        })
    }

    /// Read `ChannelMonitor`s from disk.
    pub fn read_channelmonitors<Signer: Sign, K: Deref>(
        &self,
        keys_manager: K,
    ) -> Result<Vec<(BlockHash, ChannelMonitor<Signer>)>, std::io::Error>
    where
        K::Target: KeysInterface<Signer = Signer> + Sized,
    {
        let path = self.monitors_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut res = Vec::new();
        for file_option in fs::read_dir(path)? {
            let file = file_option?;
            let owned_file_name = file.file_name();
            let filename = owned_file_name
                .to_str()
                .ok_or_else(|| invalid_data_err("Invalid ChannelMonitor file name", format!("{owned_file_name:?}")))?;
            if filename == "checkval" {
                continue;
            }
            if !filename.is_ascii() || filename.len() < 65 {
                return Err(invalid_data_err("Invalid ChannelMonitor file name", filename));
            }
            if filename.ends_with(".tmp") {
                // If we were in the middle of committing an new update and crashed, it should be
                // safe to ignore the update - we should never have returned to the caller and
                // irrevocably committed to the new state in any way.
                continue;
            }

            let txid = Txid::from_hex(filename.split_at(64).0)
                .map_err(|e| invalid_data_err("Invalid tx ID in filename error", e))?;

            let index = filename
                .split_at(65)
                .1
                .parse::<u16>()
                .map_err(|e| invalid_data_err("Invalid tx index in filename error", e))?;

            let contents = fs::read(file.path())?;
            let mut buffer = Cursor::new(&contents);
            let (blockhash, channel_monitor) = <(BlockHash, ChannelMonitor<Signer>)>::read(&mut buffer, &*keys_manager)
                .map_err(|e| invalid_data_err("Failed to deserialize ChannelMonito", e))?;

            if channel_monitor.get_funding_txo().0.txid != txid || channel_monitor.get_funding_txo().0.index != index {
                return Err(invalid_data_err(
                    "ChannelMonitor was stored in the wrong file",
                    filename,
                ));
            }

            res.push((blockhash, channel_monitor));
        }
        Ok(res)
    }
}

impl KVStorePersister for LightningFilesystemPersister {
    fn persist<W: Writeable>(&self, key: &str, object: &W) -> std::io::Result<()> {
        let mut dest_file = self.main_path();
        dest_file.push(key);
        drop_mutability!(dest_file);
        write_to_file(dest_file, object)?;

        if !matches!(key, "network_graph" | "scorer") {
            if let Some(mut dest_file) = self.backup_path() {
                dest_file.push(key);
                drop_mutability!(dest_file);
                write_to_file(dest_file, object)?;
            }
        }

        Ok(())
    }
}

#[cfg(target_family = "windows")]
macro_rules! call {
    ($e: expr) => {
        if $e != 0 {
            return Ok(());
        } else {
            return Err(std::io::Error::last_os_error());
        }
    };
}

#[cfg(target_family = "windows")]
fn path_to_windows_str<T: AsRef<OsStr>>(path: T) -> Vec<winapi::shared::ntdef::WCHAR> {
    path.as_ref().encode_wide().chain(Some(0)).collect()
}

fn write_to_file<W: Writeable>(dest_file: PathBuf, data: &W) -> std::io::Result<()> {
    let mut tmp_file = dest_file.clone();
    tmp_file.set_extension("tmp");
    drop_mutability!(tmp_file);

    // Do a crazy dance with lots of fsync()s to be overly cautious here...
    // We never want to end up in a state where we've lost the old data, or end up using the
    // old data on power loss after we've returned.
    // The way to atomically write a file on Unix platforms is:
    // open(tmpname), write(tmpfile), fsync(tmpfile), close(tmpfile), rename(), fsync(dir)
    {
        // Note that going by rust-lang/rust@d602a6b, on MacOS it is only safe to use
        // rust stdlib 1.36 or higher.
        let mut buf = BufWriter::new(fs::File::create(&tmp_file)?);
        data.write(&mut buf)?;
        buf.into_inner()?.sync_all()?;
    }
    // Fsync the parent directory on Unix.
    #[cfg(target_family = "unix")]
    {
        let parent_directory = dest_file.parent().unwrap();
        fs::rename(&tmp_file, &dest_file)?;
        let dir_file = fs::OpenOptions::new().read(true).open(parent_directory)?;
        unsafe {
            libc::fsync(dir_file.as_raw_fd());
        }
    }
    #[cfg(target_family = "windows")]
    {
        if dest_file.exists() {
            unsafe {
                winapi::um::winbase::ReplaceFileW(
                    path_to_windows_str(dest_file).as_ptr(),
                    path_to_windows_str(tmp_file).as_ptr(),
                    std::ptr::null(),
                    winapi::um::winbase::REPLACEFILE_IGNORE_MERGE_ERRORS,
                    std::ptr::null_mut() as *mut winapi::ctypes::c_void,
                    std::ptr::null_mut() as *mut winapi::ctypes::c_void,
                )
            };
        } else {
            call!(unsafe {
                winapi::um::winbase::MoveFileExW(
                    path_to_windows_str(tmp_file).as_ptr(),
                    path_to_windows_str(dest_file).as_ptr(),
                    winapi::um::winbase::MOVEFILE_WRITE_THROUGH | winapi::um::winbase::MOVEFILE_REPLACE_EXISTING,
                )
            });
        }
    }
    Ok(())
}

#[async_trait]
impl LightningStorage for LightningFilesystemPersister {
    type Error = std::io::Error;

    async fn init_fs(&self) -> Result<(), Self::Error> {
        let path = self.monitors_path();
        let backup_path = self.monitors_backup_path();
        async_blocking(move || {
            fs::create_dir_all(path.clone())?;
            if let Some(path) = backup_path {
                fs::create_dir_all(path.clone())?;
                check_dir_operations(&path)?;
                check_dir_operations(path.parent().unwrap())?;
            }
            check_dir_operations(&path)?;
            check_dir_operations(path.parent().unwrap())
        })
        .await
    }

    async fn is_fs_initialized(&self) -> Result<bool, Self::Error> {
        let dir_path = self.monitors_path();
        let backup_dir_path = self.monitors_backup_path();
        async_blocking(move || {
            if !dir_path.exists() || backup_dir_path.as_ref().map(|path| !path.exists()).unwrap_or(false) {
                Ok(false)
            } else if !dir_path.is_dir() {
                Err(std::io::Error::other(format!(
                    "{} is not a directory",
                    dir_path.display()
                )))
            } else if backup_dir_path.as_ref().map(|path| !path.is_dir()).unwrap_or(false) {
                Err(std::io::Error::other("Backup path is not a directory"))
            } else {
                let check_backup_ops = if let Some(backup_path) = backup_dir_path {
                    check_dir_operations(&backup_path).is_ok()
                } else {
                    true
                };
                check_dir_operations(&dir_path).map(|_| check_backup_ops)
            }
        })
        .await
    }

    async fn get_nodes_addresses(&self) -> Result<NodesAddressesMap, Self::Error> {
        let path = self.nodes_addresses_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }

        let nodes_addresses: HashMap<String, SocketAddr> = read_json(&path)
            .await
            .map_err(|e| invalid_data_err("Error", e))?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;

        nodes_addresses
            .iter()
            .map(|(pubkey_str, addr)| {
                let pubkey = PublicKey::from_str(pubkey_str).map_err(|e| invalid_data_err("Error", e))?;
                Ok((pubkey, *addr))
            })
            .collect()
    }

    async fn save_nodes_addresses(&self, nodes_addresses: NodesAddressesMapShared) -> Result<(), Self::Error> {
        let path = self.nodes_addresses_path();
        let backup_path = self.nodes_addresses_backup_path();

        let nodes_addresses: HashMap<String, SocketAddr> = nodes_addresses
            .lock()
            .iter()
            .map(|(pubkey, addr)| (pubkey.to_string(), *addr))
            .collect();

        write_json(&nodes_addresses, &path, USE_TMP_FILE)
            .await
            .map_err(|e| invalid_data_err("Error", e))?;

        if let Some(path) = backup_path {
            write_json(&nodes_addresses, &path, USE_TMP_FILE)
                .await
                .map_err(|e| invalid_data_err("Error", e))?;
        }

        Ok(())
    }

    async fn get_network_graph(&self, network: Network, logger: Arc<LogState>) -> Result<NetworkGraph, Self::Error> {
        let path = self.network_graph_path();
        if !path.exists() {
            return Ok(NetworkGraph::new(genesis_block(network).header.block_hash(), logger));
        }
        async_blocking(move || {
            let file = fs::File::open(path)?;
            common::log::info!("Reading the saved lightning network graph from file, this can take some time!");
            NetworkGraph::read(&mut BufReader::new(file), logger).map_err(|e| invalid_data_err("Error", e))
        })
        .await
    }

    async fn get_scorer(&self, network_graph: Arc<NetworkGraph>, logger: Arc<LogState>) -> Result<Scorer, Self::Error> {
        let path = self.scorer_path();
        if !path.exists() {
            return Ok(Mutex::new(ProbabilisticScorer::new(
                ProbabilisticScoringParameters::default(),
                network_graph,
                logger,
            )));
        }
        async_blocking(move || {
            let file = fs::File::open(path)?;
            let scorer = ProbabilisticScorer::read(
                &mut BufReader::new(file),
                (ProbabilisticScoringParameters::default(), network_graph, logger),
            )
            .map_err(|e| invalid_data_err("Error", e))?;
            Ok(Mutex::new(scorer))
        })
        .await
    }

    async fn get_trusted_nodes(&self) -> Result<HashSet<PublicKey>, Self::Error> {
        let path = self.trusted_nodes_path();
        if !path.exists() {
            return Ok(HashSet::new());
        }

        let trusted_nodes: HashSet<String> = read_json(&path)
            .await
            .map_err(|e| invalid_data_err("Error", e))?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;

        trusted_nodes
            .iter()
            .map(|pubkey_str| {
                let pubkey = PublicKey::from_str(pubkey_str).map_err(|e| invalid_data_err("Error", e))?;
                Ok(pubkey)
            })
            .collect()
    }

    async fn save_trusted_nodes(&self, trusted_nodes: TrustedNodesShared) -> Result<(), Self::Error> {
        let path = self.trusted_nodes_path();
        let trusted_nodes: HashSet<String> = trusted_nodes.lock().iter().map(|pubkey| pubkey.to_string()).collect();
        write_json(&trusted_nodes, &path, USE_TMP_FILE)
            .await
            .map_err(|e| invalid_data_err("Error", e))
    }
}
