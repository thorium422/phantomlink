use std::process::Command;

use log::{debug, warn};
use uom::si::{f64::InformationRate, information_rate::kilobit_per_second};

use crate::phork::namespace::{HTB_AQM_HANDLE, HTB_LEAF_CLASSID, NS_NAME_LINK};

pub fn freeze_htb(link_id: usize) {
    // HTB won't let us do 0, so we set it to the minimum HTB will accept
    update_htb_rate(link_id, InformationRate::new::<kilobit_per_second>(1.0));
}

pub fn update_htb_rate(link_id: usize, rate: InformationRate) {
    let iface = format!("pqueue{link_id}_in");
    let kbit = rate.get::<kilobit_per_second>();
    // HTB doesn't accept 0 as a rate, so we have to clamp it just to make sure
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
