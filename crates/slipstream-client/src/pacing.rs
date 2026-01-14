use slipstream_ffi::picoquic::{
    get_bytes_in_transit, get_cwin, get_pacing_rate, get_rtt, picoquic_cnx_t,
};

// Pacing gain tuning for the poll-based pacing loop.
const PACING_GAIN_BASE: f64 = 1.0;
const PACING_GAIN_PROBE: f64 = 1.25;
const PACING_GAIN_EPSILON: f64 = 0.05;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PacingBudgetSnapshot {
    pub(crate) pacing_rate: u64,
    pub(crate) qps: f64,
    pub(crate) gain: f64,
    pub(crate) target_inflight: usize,
}

pub(crate) struct PacingPollBudget {
    payload_bytes: f64,
    mtu: u32,
    last_pacing_rate: u64,
}

impl PacingPollBudget {
    pub(crate) fn new(mtu: u32) -> Self {
        debug_assert!(mtu > 0, "PacingPollBudget::new expects MTU > 0");
        Self {
            payload_bytes: mtu.max(1) as f64,
            mtu,
            last_pacing_rate: 0,
        }
    }

    pub(crate) fn target_inflight(
        &mut self,
        cnx: *mut picoquic_cnx_t,
        rtt_proxy_us: u64,
    ) -> PacingBudgetSnapshot {
        let pacing_rate = unsafe { get_pacing_rate(cnx) };
        let rtt_seconds = (self.derive_rtt_us(cnx, rtt_proxy_us) as f64) / 1_000_000.0;
        if pacing_rate == 0 {
            let target_inflight = cwnd_target_polls(cnx, self.mtu);
            let qps = target_inflight as f64 / rtt_seconds;
            self.last_pacing_rate = 0;
            return PacingBudgetSnapshot {
                pacing_rate,
                qps,
                gain: PACING_GAIN_BASE,
                target_inflight,
            };
        }

        let gain = self.next_gain(pacing_rate);
        let qps = (pacing_rate as f64 / self.payload_bytes) * gain;
        let target_inflight = (qps * rtt_seconds).ceil().min(usize::MAX as f64) as usize;

        PacingBudgetSnapshot {
            pacing_rate,
            qps,
            gain,
            target_inflight,
        }
    }

    fn derive_rtt_us(&self, cnx: *mut picoquic_cnx_t, rtt_proxy_us: u64) -> u64 {
        let smoothed = unsafe { get_rtt(cnx) };
        let candidate = if smoothed > 0 { smoothed } else { rtt_proxy_us };
        // Clamp to 1us to avoid divide-by-zero when RTT is unknown.
        candidate.max(1)
    }

    fn next_gain(&mut self, pacing_rate: u64) -> f64 {
        let gain =
            if pacing_rate as f64 > (self.last_pacing_rate as f64) * (1.0 + PACING_GAIN_EPSILON) {
                PACING_GAIN_PROBE
            } else {
                PACING_GAIN_BASE
            };
        self.last_pacing_rate = pacing_rate;
        gain
    }
}

pub(crate) fn cwnd_target_polls(cnx: *mut picoquic_cnx_t, mtu: u32) -> usize {
    debug_assert!(mtu > 0, "mtu must be > 0");
    let mtu = mtu as u64;
    if mtu == 0 {
        return 0;
    }
    let cwnd = unsafe { get_cwin(cnx) };
    let target = cwnd.saturating_add(mtu - 1) / mtu;
    usize::try_from(target).unwrap_or(usize::MAX)
}

pub(crate) fn inflight_packet_estimate(cnx: *mut picoquic_cnx_t, mtu: u32) -> usize {
    debug_assert!(mtu > 0, "mtu must be > 0");
    let mtu = mtu as u64;
    if mtu == 0 {
        return 0;
    }
    let inflight = unsafe { get_bytes_in_transit(cnx) };
    let packets = inflight.saturating_add(mtu - 1) / mtu;
    if packets > usize::MAX as u64 {
        usize::MAX
    } else {
        packets as usize
    }
}
