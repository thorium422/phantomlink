use eyre;
use libc::{setns, CLONE_NEWNET};
use log::info;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::opt::ShaperKind;
use crate::phork::utils::*;
use crate::phork::veth::VEth;

/// Placeholder rate written into the HTB class at `setup` time. The runtime
/// overwrites this on the first scenario tick, so any sensible value works.
/// We pick 1 Gbit/s to make accidental no-runtime usage obviously
/// over-provisioned rather than throttling.
const HTB_PLACEHOLDER_RATE_KBIT: u32 = 1_000_000;
/// Class id of the (only) HTB leaf class on `pqueueN_in`. The runtime locates
/// it by this exact id, so don't change without updating
/// `crate::queue::qdisc_shaper`.
pub(crate) const HTB_LEAF_CLASSID: &str = "1:10";
/// Handle of the AQM child qdisc under the HTB leaf class.
pub(crate) const HTB_AQM_HANDLE: &str = "10:";

pub const NS_NAME_LINK: &str = "pl_link";
pub const NS_NAME_CLIENT: &str = "pl_client";
pub const NS_NAME_SERVER: &str = "pl_server";
const NAMESPACES: [&str; 3] = [NS_NAME_CLIENT, NS_NAME_LINK, NS_NAME_SERVER];
const VETH_1: &str = "veth1";
const VETH_2: &str = "veth2";

/// Checks if the network environment is set up.
pub(crate) fn is_setup() -> eyre::Result<bool> {
    for ns in NAMESPACES {
        if Namespace::try_exists(ns)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sets up the network namespaces and virtual ethernet links for the phork environment.
pub(crate) fn setup(
    qdisc_client_config: Vec<String>,
    qdisc_server_config: Vec<String>,
    qdisc_client_shaper: ShaperKind,
    qdisc_server_shaper: ShaperKind,
) -> eyre::Result<()> {
    // create namespaces
    for ns in NAMESPACES {
        Namespace::try_create(ns)?;
    }

    // configure virtual ethernet links
    let veths = [
        VEth::new(
            VETH_1,
            NS_NAME_CLIENT,
            "192.168.22.2/24",
            "22:22:22:22:22:22",
            "192.168.33.2/24",
            "44:22:22:22:22:22",
        ),
        VEth::new(
            VETH_2,
            NS_NAME_SERVER,
            "192.168.66.2/24",
            "66:66:66:66:66:66",
            "192.168.33.3/24",
            "44:66:66:66:66:66",
        ),
    ];

    // create virtual ethernet link
    for veth in &veths {
        veth.create()?;
    }

    // configure interfaces
    for veth in veths {
        veth.set_interfaces_up()?;
        veth.set_loopback_up()?;
        veth.disable_offload()?;
        veth.set_default_route()?;
    }

    // Create qdisc veth pairs for each link
    setup_qdisc_veths(
        qdisc_client_config.iter().map(|x| x.as_str()).collect(),
        qdisc_server_config.iter().map(|x| x.as_str()).collect(),
        qdisc_client_shaper,
        qdisc_server_shaper,
    )?;

    // assuming the default namespace is the one with ID 1
    let ns_id = 1;
    exec("ln", &["-sf", &format!("/proc/{ns_id}/ns/net"), "/var/run/netns/default"])?;

    Ok(())
}

/// Cleans up the network namespaces and virtual ethernet links created by the setup function.
pub(crate) fn clean() -> eyre::Result<()> {
    for ns in NAMESPACES {
        match Namespace::try_load(ns) {
            Ok(namespace) => {
                namespace.try_delete()?;
            }
            Err(e) => {
                if e.to_string().contains("does not exist") {
                    // Namespace does not exist, nothing to delete
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    }

    // Clean up default namespace symlink
    let default_ns_path = Path::new("/var/run/netns/default");
    if default_ns_path.exists() {
        std::fs::remove_file(default_ns_path)?;
    }
    Ok(())
}

/// Represents a network namespace.
pub struct Namespace {
    name: String,
}

impl Namespace {
    /// Creates a new Namespace instance with the given name.
    pub fn try_create(name: &str) -> eyre::Result<Self> {
        if Namespace::try_exists(name)? {
            return Err(eyre::eyre!("Namespace '{}' already exists", name));
        }
        netns(&["add", name])?;
        Ok(Namespace { name: name.to_string() })
    }

    /// Loads an existing Namespace instance by name.
    pub fn try_load(name: &str) -> eyre::Result<Self> {
        if !Namespace::try_exists(name)? {
            return Err(eyre::eyre!("Namespace '{}' does not exist", name));
        }
        Ok(Namespace { name: name.to_string() })
    }

    /// Checks if a namespace with the given name exists.
    pub fn try_exists(name: &str) -> eyre::Result<bool> {
        Ok(Namespace { name: name.to_string() }.path().try_exists()?)
    }

    /// Deletes the namespace.
    pub fn try_delete(self) -> eyre::Result<()> {
        netns(&["del", &self.name])?;
        Ok(())
    }

    /// Moves the calling process to the network namespace.
    pub fn try_switch_calling_pid_to_namespace(&self) -> eyre::Result<()> {
        let ns_path = self.path();
        let ns_fd = File::open(ns_path).map_err(|e| eyre::eyre!("Failed to open namespace file: {}", e))?;
        unsafe {
            if setns(ns_fd.as_raw_fd(), CLONE_NEWNET) == -1 {
                return Err(eyre::eyre!("Failed to set network namespace: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    /// Returns the fs path to the namespace.
    fn path(&self) -> PathBuf {
        Path::new(&format!("/var/run/netns/{}", self.name)).to_path_buf()
    }
}

fn setup_qdisc_veths(
    qdisc_client_config: Vec<&str>,
    qdisc_server_config: Vec<&str>,
    qdisc_client_shaper: ShaperKind,
    qdisc_server_shaper: ShaperKind,
) -> eyre::Result<()> {
    let qdisc_veths = [
        ("pqueue0", qdisc_client_config, qdisc_client_shaper),
        ("pqueue1", qdisc_server_config, qdisc_server_shaper),
    ];

    for (veth_name, qdisc_config, shaper) in &qdisc_veths {
        let in_name = format!("{veth_name}_in");
        let out_name = format!("{veth_name}_out");

        // Cleanup existing interface if it exists
        let _ = Command::new("ip")
            .args(["netns", "exec", NS_NAME_LINK, "ip", "link", "del", &in_name])
            .output();

        // Create veth pair
        Command::new("ip")
            .args([
                "netns",
                "exec",
                NS_NAME_LINK,
                "ip",
                "link",
                "add",
                &in_name,
                "type",
                "veth",
                "peer",
                "name",
                &out_name,
            ])
            .status()?;

        Command::new("ip")
            .args(["netns", "exec", NS_NAME_LINK, "ip", "link", "set", &in_name, "up"])
            .status()?;

        Command::new("ip")
            .args(["netns", "exec", NS_NAME_LINK, "ip", "link", "set", &out_name, "up"])
            .status()?;

        match shaper {
            ShaperKind::None => {
                let mut qdisc_args = vec!["netns", "exec", NS_NAME_LINK, "tc", "qdisc", "add", "dev", &in_name, "root"];
                qdisc_args.extend_from_slice(qdisc_config);
                Command::new("ip").args(qdisc_args).status()?;
            }
            ShaperKind::Htb => {
                // See docs/qdisc-design-rationale.md: classless AQMs are no-ops as root in
                // phantomlink's topology because the inner veth has no rate limit. We wrap
                // them in an HTB shaper whose rate the scenario engine updates each tick.
                let placeholder = format!("{HTB_PLACEHOLDER_RATE_KBIT}kbit");

                Command::new("ip")
                    .args([
                        "netns",
                        "exec",
                        NS_NAME_LINK,
                        "tc",
                        "qdisc",
                        "add",
                        "dev",
                        &in_name,
                        "root",
                        "handle",
                        "1:",
                        "htb",
                        "default",
                        "10",
                    ])
                    .status()?;
                Command::new("ip")
                    .args([
                        "netns",
                        "exec",
                        NS_NAME_LINK,
                        "tc",
                        "class",
                        "add",
                        "dev",
                        &in_name,
                        "parent",
                        "1:",
                        "classid",
                        HTB_LEAF_CLASSID,
                        "htb",
                        "rate",
                        &placeholder,
                        "ceil",
                        &placeholder,
                    ])
                    .status()?;
                let mut aqm_args = vec![
                    "netns",
                    "exec",
                    NS_NAME_LINK,
                    "tc",
                    "qdisc",
                    "add",
                    "dev",
                    &in_name,
                    "parent",
                    HTB_LEAF_CLASSID,
                    "handle",
                    HTB_AQM_HANDLE,
                ];
                aqm_args.extend_from_slice(qdisc_config);
                Command::new("ip").args(aqm_args).status()?;

                info!(
                    "Built HTB+AQM tree on {in_name} (leaf classid {HTB_LEAF_CLASSID}, AQM handle {HTB_AQM_HANDLE}, placeholder rate {placeholder}; runtime will update on each scenario tick)"
                );
            }
        }
    }

    Ok(())
}
