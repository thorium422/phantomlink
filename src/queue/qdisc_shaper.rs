//! Runtime control of the HTB shaper that wraps each direction's AQM.
//!
//! See `docs/qdisc-design-rationale.md` for the architectural background.
//! `phork::namespace::setup_qdisc_veths` always builds an HTB+AQM tree on
//! `pqueueN_in` with the AQM as the child of HTB's leaf class. This module is
//! responsible for telling that HTB class what rate to enforce.
//!
//! Rate updates are issued by shelling out to `tc`. At the scenario cadence
//! the paper targets (updates every 11 s) the syscall cost is negligible; if
//! a future scenario needs sub-second updates this is the place to swap in a
//! direct `rtnetlink` client.

use std::process::Command;

use log::{debug, warn};
use uom::si::{f64::InformationRate, information_rate::kilobit_per_second};

use crate::phork::namespace::{HTB_AQM_HANDLE, HTB_LEAF_CLASSID, NS_NAME_LINK};

/// Sets the HTB leaf class rate to the minimum HTB will accept, effectively
/// halting the shaper for the duration of a reconfiguration window. The
/// caller is responsible for restoring the rate after the window expires
/// (typically by calling `update_htb_rate` from a spawned timer thread).
pub fn freeze_htb(link_id: usize) {
    update_htb_rate(link_id, InformationRate::new::<kilobit_per_second>(1.0));
}

/// Updates the HTB leaf class on `pqueue<link_id>_in` to enforce `rate`.
///
/// Logs but does not propagate failures: a missed rate update reverts to the
/// previous rate (still close to correct for slow scenario changes), whereas
/// crashing the OVL loop would lose the whole run.
pub fn update_htb_rate(link_id: usize, rate: InformationRate) {
    let iface = format!("pqueue{link_id}_in");
    let kbit = rate.get::<kilobit_per_second>();
    // HTB's minimum rate is a few bits/sec; clamp aggressively to avoid
    // tc rejecting tiny rates with EINVAL when a scenario flips to zero.
    let kbit_clamped = if kbit < 1.0 { 1.0 } else { kbit };
    let rate_str = format!("{:.0}kbit", kbit_clamped.round());

    let result = Command::new("ip")
        .args([
            "netns",
            "exec",
            NS_NAME_LINK,
            "tc",
            "class",
            "change",
            "dev",
            &iface,
            "parent",
            "1:",
            "classid",
            HTB_LEAF_CLASSID,
            "htb",
            "rate",
            &rate_str,
            "ceil",
            &rate_str,
        ])
        .output();

    match result {
        Ok(o) if o.status.success() => {
            debug!("Updated HTB class {HTB_LEAF_CLASSID} on {iface} (AQM under handle {HTB_AQM_HANDLE}): rate={rate_str}");
        }
        Ok(o) => {
            warn!(
                "tc class change on {} failed: status={:?} stderr={}",
                iface,
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        Err(e) => {
            warn!("tc class change on {iface} failed to spawn: {e}");
        }
    }
}
