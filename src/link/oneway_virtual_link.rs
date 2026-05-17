use std::{
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

use core_affinity::{set_for_current, CoreId};
use crossbeam::atomic::AtomicCell;
use eyre::{OptionExt, Result};
use log::debug;
use pnet::datalink::{DataLinkReceiver, DataLinkSender};
use spin_sleep::SpinSleeper;
use thread_priority::{set_current_thread_priority, ThreadPriority};
use uom::si::{
    f64::{Information, InformationRate, Time},
    information::kilobyte,
    information_rate::megabit_per_second,
    time::microsecond,
};

use crate::{
    cli::StartupMode,
    inflight_queue::InflightQueue,
    queue::{qdisc_shaper, qdisc_wrapper::qdisc_channel},
    route_metrics::RouteMetricQueue,
    runtime::Runtime,
    ReconfigurationMode,
};

use super::{deliverer::Deliverer, drainer::Drainer, pacer::Pacer};

pub struct OVLCoreConfig {
    core_id_ovl: CoreId,
    core_id_deliverer: CoreId,
    core_id_drainer: CoreId,
    core_id_pacer: CoreId,
}

impl OVLCoreConfig {
    pub fn new_from_vec(mut core_vec: Vec<CoreId>) -> OVLCoreConfig {
        assert!(core_vec.len() >= 4);
        OVLCoreConfig {
            core_id_ovl: core_vec.pop().expect("vector length >= 4"),
            core_id_deliverer: core_vec.pop().expect("vector length >= 4"),
            core_id_drainer: core_vec.pop().expect("vector length >= 4"),
            core_id_pacer: core_vec.pop().expect("vector length >= 4"),
        }
    }
}

pub struct OnewayVirtualLink {
    link_id: usize,
    startup_mode: StartupMode,
    buffer_size_multiplier: f64,
    core_config: Option<OVLCoreConfig>,
}

impl OnewayVirtualLink {
    pub fn new(
        link_id: usize,
        startup_mode: StartupMode,
        buffer_size_multiplier: f64,
        core_config: Option<OVLCoreConfig>,
    ) -> OnewayVirtualLink {
        OnewayVirtualLink {
            link_id,
            startup_mode,
            buffer_size_multiplier,
            core_config,
        }
    }

    pub fn run(
        &mut self,
        input: Box<dyn DataLinkReceiver>,
        output: Box<dyn DataLinkSender>,
        mut route_metric_queue: RouteMetricQueue,
        reconfiguration_delay: Duration,
        reconfiguration_mode: ReconfigurationMode,
    ) -> Result<()> {
        let core_id_ovl = self.core_config.as_ref().map(|cfg| cfg.core_id_ovl);
        if let Some(core_id_ovl) = core_id_ovl {
            debug!("Link {}: start one-way virtual link (Core: {}).", self.link_id, core_id_ovl.id);
            set_for_current(core_id_ovl);
        } else {
            debug!("Link {}: start one-way virtual link.", self.link_id);
        }
        set_current_thread_priority(ThreadPriority::Max).expect("Could not set thread priority to MAX.");

        // create packet stacks for this link
        let spin_sleep = SpinSleeper::default();
        let rdp = route_metric_queue.pop_next_route_metric().ok_or_eyre("Could not get initial RDP")?;
        let btldr: InformationRate = rdp.btldr;
        let delay = rdp.delay; // initial delay
        let mut route_id = rdp.route_id;
        let inflight_queue = Arc::new((Mutex::new(InflightQueue::new(self.link_id, delay)), Condvar::new()));

        // Probe once whether setup built an HTB shaper on pqueueN_in. When it
        // did, we mirror every pacer rate change onto the leaf class. When it
        // didn't (the default), we leave the tc tree alone and behave exactly
        // as before. See docs/qdisc-design-rationale.md for the design.
        let has_htb_shaper = qdisc_shaper::has_htb_shaper(self.link_id);
        if has_htb_shaper {
            debug!("Link {}: HTB shaper detected on pqueue{}_in; slaving its rate to pacer", self.link_id, self.link_id);
            qdisc_shaper::update_htb_rate(self.link_id, btldr);
        }

        let (sender, receiver, _qdisc_handle) = qdisc_channel(&format!("pqueue{}", self.link_id))?;

        // create & start drainer
        let drainer = Arc::new(Drainer::new(self.link_id, self.startup_mode, sender));
        let drainer_clone = drainer.clone();
        let core_id_drainer = self.core_config.as_ref().map(|cfg| cfg.core_id_drainer);
        let thread_drainer = thread::spawn(move || {
            drainer_clone.run(input, core_id_drainer);
        });

        // create & start pacer
        let pacer = Arc::new(Pacer::create(route_id, receiver, inflight_queue.clone(), btldr, delay));
        let pacer_clone: Arc<Pacer<_>> = pacer.clone();
        let core_id_pacer = self.core_config.as_ref().map(|cfg| cfg.core_id_pacer);
        let thread_pacer = thread::spawn(move || {
            pacer_clone.run(core_id_pacer);
        });

        // create & start sender
        let inflight_queue_sender = inflight_queue.clone();
        let deliverer_reconfig_until = Arc::new(AtomicCell::new(None));
        let mut deliverer = Deliverer::new(self.link_id, inflight_queue_sender, deliverer_reconfig_until.clone());
        let core_id_deliverer = self.core_config.as_ref().map(|cfg| cfg.core_id_deliverer);
        let thread_sender = thread::spawn(move || {
            deliverer.run(output, core_id_deliverer);
        });

        loop {
            if !Runtime::is_app_start_time_initialized() {
                // info!("Waiting for first packet");
                spin_sleep.sleep_ns(10_000);
                continue;
            }

            let time_since_start = OnewayVirtualLink::time_since_start();
            let next_route_metric = match route_metric_queue.peek_next_route_metric() {
                Some(rm) => rm,
                None => {
                    debug!("Found last route metric");
                    break;
                }
            };

            // if we still need to wait for next event
            if next_route_metric.time_after_start > time_since_start {
                let wait_time = next_route_metric.time_after_start - time_since_start;
                debug!("Wait {}ms for next route metric.", wait_time.as_millis());
                spin_sleep.sleep_ns(wait_time.as_nanos().try_into().unwrap());
            } else {
                let route_metric = route_metric_queue.pop_next_route_metric().ok_or_eyre("Could not get next RDP")?;
                let btldr: InformationRate = route_metric.btldr;
                let delay = route_metric.delay;
                debug!(
                    "{}ms since start: running metric {}-{}",
                    time_since_start.as_millis(),
                    btldr.get::<megabit_per_second>().round(),
                    delay.as_millis()
                );
                debug!(
                    "Update route, set delay={} ms and btldr={} Mbps",
                    delay.as_millis(),
                    btldr.get::<megabit_per_second>().round()
                );
                // Keep the qdisc as the narrower of (qdisc, pacer) throughout
                // the transition so that the AQM keeps owning the drops:
                //   - widening (new > current): pacer first, qdisc second
                //   - narrowing/equal:           qdisc first, pacer second
                // See docs/qdisc-design-rationale.md "How this preserves P3".
                if has_htb_shaper {
                    let current = pacer.current_datarate();
                    if btldr > current {
                        pacer.update_datarate(btldr);
                        qdisc_shaper::update_htb_rate(self.link_id, btldr);
                    } else {
                        qdisc_shaper::update_htb_rate(self.link_id, btldr);
                        pacer.update_datarate(btldr);
                    }
                } else {
                    pacer.update_datarate(btldr);
                }
                pacer.update_delay(delay);

                let new_route_id = route_metric.route_id;
                if route_id != new_route_id {
                    debug!("Switch route {}->{}", route_id, new_route_id);
                    route_id = new_route_id;

                    // GSL (Pacer: Ground - Satellite)
                    pacer.switch_route(new_route_id, reconfiguration_delay);
                    // ISL (Inflight Queue)
                    if let ReconfigurationMode::All = reconfiguration_mode {
                        let mut iq = inflight_queue.0.lock().unwrap();
                        iq.switch_route(reconfiguration_delay);
                    }
                    // GSL (Deliverer: Satellite - Ground)
                    deliverer_reconfig_until.store(Some(Instant::now() + reconfiguration_delay));

                    // Mirror the pacer's reconfiguration blackout onto the HTB
                    // shaper. Without this, packets keep dequeueing through the
                    // qdisc at the configured rate during the window, pile up on
                    // the inner veth peer's rx queue (which the pacer stops
                    // draining), and drop silently — re-introducing the
                    // backpressure-decoupling failure mode the HTB wrapper exists
                    // to avoid. See docs/qdisc-design-rationale.md P2.
                    // The restore reads the pacer's current rate (not the rate
                    // captured at switch time) so a scenario tick that fires
                    // *during* the window is honoured rather than clobbered.
                    if has_htb_shaper && !reconfiguration_delay.is_zero() {
                        qdisc_shaper::freeze_htb(self.link_id);
                        let link_id = self.link_id;
                        let pacer_for_restore = pacer.clone();
                        let window = reconfiguration_delay;
                        thread::spawn(move || {
                            thread::sleep(window);
                            qdisc_shaper::update_htb_rate(link_id, pacer_for_restore.current_datarate());
                        });
                    }
                }
            }
        }

        thread_drainer.join().unwrap();
        thread_pacer.join().unwrap();
        thread_sender.join().unwrap();
        Ok(())
    }

    #[allow(dead_code)]
    fn calculate_bottleneck_buffer_size(&self, datarate: InformationRate, delay: Duration) -> Information {
        let bdp: Information = Self::calculate_bdp(datarate, delay);
        let buffer_size: Information = bdp * self.buffer_size_multiplier;
        debug!(
            "Link {}: Set BDP={} kB, BtlBufferSize={} kB",
            self.link_id,
            bdp.get::<kilobyte>().round(),
            buffer_size.get::<kilobyte>().round()
        );
        buffer_size
    }

    fn time_since_start() -> Duration {
        Instant::now() - Runtime::access_app_start_time()
    }

    pub fn calculate_bdp(datarate: InformationRate, delay: Duration) -> Information {
        let rtt: Time = Time::new::<microsecond>(delay.as_micros() as f64 * 2.0);
        (datarate * rtt).into()
    }
}
