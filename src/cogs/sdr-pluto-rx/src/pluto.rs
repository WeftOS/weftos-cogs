//! The live Pluto: an AD9363 (or, with the firmware's AD9364 setting, 70 to 6000 MHz) behind
//! libiio, receive only.
//!
//! On taking ownership the cog puts the chip in a known receive-only state: digital loopback off
//! (a crashed client can leave it on, and the receive path then reads zeros), the DDS tones off
//! and the TX attenuation at its maximum. It never enables a transmit buffer.

use crate::iio::{Buffer, Channel, Context, Device};
use crate::plan::{Band, Gain};
use crate::radio::{HwStatus, Radio, RefillError, SafeState};

/// The lowest sampling rate the chip reaches without its FIR decimating.
const NO_FIR_MIN_RATE: u32 = 2_083_334;
const TX_ATTEN_MAX_DB: f64 = -89.75;

pub struct Pluto {
    // Field order is drop order: the buffer, then the handles, then the context.
    buf: Option<Buffer>,
    rx_i: Channel,
    rx_q: Channel,
    phy_rx: Channel,
    phy_tx: Channel,
    rx_lo: Channel,
    temp: Option<Channel>,
    xadc_temp: Option<Channel>,
    rx: Device,
    phy: Device,
    dds: Option<Device>,
    ctx: Context,
    block: usize,
    cur: Option<Band>,
    fir_dec: Option<u32>,
    safe: SafeState,
}

/// A temperature the sensors can really report. The AD9361's sensor read -80 and -75 degrees C
/// after the Pluto came back from a reset on the overnight soak; such a value is dropped, not
/// published.
fn plausible_temp(t: f64) -> Option<f64> {
    (-40.0..=125.0).contains(&t).then_some(t)
}

fn num(s: &str) -> Option<f64> {
    s.split_whitespace().next()?.parse().ok()
}

/// A FIR for decimation `dec` (2 or 4) in the format the ad9361 driver loads: a lowpass at 40
/// percent of the output rate. The transmit side gets the matching interpolation filter so the
/// chip accepts the pair; nothing is transmitted.
pub fn fir_config(dec: u32) -> String {
    let taps = 64usize;
    let fc = 0.4 / f64::from(dec);
    let mut c = Vec::with_capacity(taps);
    for n in 0..taps {
        let m = n as f64 - (taps as f64 - 1.0) / 2.0;
        let h = if m == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * m).sin() / (std::f64::consts::PI * m)
        };
        let w = 0.54 - 0.46 * (2.0 * std::f64::consts::PI * n as f64 / (taps as f64 - 1.0)).cos();
        c.push(h * w);
    }
    let sum: f64 = c.iter().sum();
    let mut s = format!("RX 3 GAIN 0 DEC {dec}\nTX 3 GAIN -6 INT {dec}\n");
    for x in c {
        let q = (x / sum * 32767.0 * 0.9).round() as i32;
        s.push_str(&format!("{q},{q}\n"));
    }
    s
}

/// The decimation a rate needs, if any.
pub fn fir_for(rate: u32) -> Option<u32> {
    if rate >= NO_FIR_MIN_RATE {
        None
    } else if rate >= NO_FIR_MIN_RATE / 2 {
        Some(2)
    } else {
        Some(4)
    }
}

impl Pluto {
    pub fn open(uri: &str, block: usize) -> Result<Pluto, String> {
        let ctx = Context::open(uri, 10_000)?;
        let phy = ctx.device("ad9361-phy")?;
        let rx = ctx.device("cf-ad9361-lpc")?;
        let dds = ctx.device("cf-ad9361-dds-core-lpc").ok();
        let xadc = ctx.device("xadc").ok();
        let mut p = Pluto {
            buf: None,
            rx_i: rx.channel("voltage0", false)?,
            rx_q: rx.channel("voltage1", false)?,
            phy_rx: phy.channel("voltage0", false)?,
            phy_tx: phy.channel("voltage0", true)?,
            rx_lo: phy.channel("altvoltage0", true)?,
            temp: phy.channel("temp0", false).ok(),
            xadc_temp: xadc.and_then(|d| d.channel("temp0", false).ok()),
            rx,
            phy,
            dds,
            ctx,
            block,
            cur: None,
            fir_dec: None,
            safe: SafeState::default(),
        };
        p.fir_dec = match p.phy_rx.attr("filter_fir_en").as_deref() {
            Ok("1") => Some(0),
            _ => None,
        };
        p.take_ownership();
        Ok(p)
    }

    /// Loopback off, DDS tones off, TX attenuation at maximum; what was found is recorded.
    fn take_ownership(&mut self) {
        let mut s = SafeState {
            loopback_was: self.phy.debug_attr("loopback").ok(),
            ..SafeState::default()
        };
        match self.phy.set_debug_attr("loopback", "0") {
            Ok(()) => {}
            Err(e) => s.notes.push(format!("loopback not reset: {e}")),
        }
        s.loopback_now = self.phy.debug_attr("loopback").ok();
        s.tx_atten_was_db = self
            .phy_tx
            .attr("hardwaregain")
            .ok()
            .as_deref()
            .and_then(num);
        if let Err(e) = self
            .phy_tx
            .set_attr("hardwaregain", &format!("{TX_ATTEN_MAX_DB}"))
        {
            s.notes.push(format!("TX attenuation not set: {e}"));
        }
        s.tx_atten_now_db = self
            .phy_tx
            .attr("hardwaregain")
            .ok()
            .as_deref()
            .and_then(num);
        let mut off = self.dds.is_some();
        if let Some(dds) = self.dds {
            for i in 0..4 {
                match dds.channel(&format!("altvoltage{i}"), true) {
                    Ok(ch) => {
                        let _ = ch.set_attr("scale", "0.000000");
                        if ch.set_attr("raw", "0").is_err() {
                            off = false;
                            s.notes.push(format!("DDS tone {i} not switched off"));
                        }
                    }
                    Err(e) => s.notes.push(e),
                }
            }
        }
        s.dds_tones_off = off;
        self.safe = s;
    }

    /// The chip wants the receive and the transmit FIR switched together (the config holds both);
    /// libiio presents them as two attributes, one per direction.
    fn set_fir_en(&self, on: bool) -> Result<(), String> {
        let v = if on { "1" } else { "0" };
        self.phy_tx.set_attr("filter_fir_en", v)?;
        self.phy_rx.set_attr("filter_fir_en", v)
    }

    fn ensure_fir(&mut self, dec: u32) -> Result<(), String> {
        if self.fir_dec == Some(dec) {
            return Ok(());
        }
        // The chip takes a new FIR only while the old one is off, at a rate it reaches without it.
        self.phy_rx.set_attr("sampling_frequency", "4000000")?;
        self.set_fir_en(false)?;
        self.phy.set_attr("filter_fir_config", &fir_config(dec))?;
        self.set_fir_en(true)?;
        self.fir_dec = Some(dec);
        Ok(())
    }

    fn set_if_changed(
        ch: &Channel,
        attr: &str,
        want: &str,
        have: Option<&str>,
    ) -> Result<(), String> {
        if have == Some(want) {
            return Ok(());
        }
        ch.set_attr(attr, want)
    }
}

impl Radio for Pluto {
    fn describe(&self) -> String {
        format!("PlutoSDR on {}", self.ctx.uri)
    }

    fn block_samples(&self) -> usize {
        self.block
    }

    fn apply(&mut self, b: &Band) -> Result<(), String> {
        b.check()?;
        // No sample of the old tuning may be delivered after this returns.
        self.buf = None;
        let cur = self.cur.clone();
        let rate_changed = cur.as_ref().is_none_or(|c| c.rate_sps != b.rate_sps);
        if rate_changed {
            match fir_for(b.rate_sps) {
                Some(dec) => {
                    self.ensure_fir(dec)?;
                    self.phy_rx
                        .set_attr("sampling_frequency", &b.rate_sps.to_string())?;
                }
                None => {
                    self.phy_rx
                        .set_attr("sampling_frequency", &b.rate_sps.to_string())?;
                    if self.fir_dec.is_some() {
                        self.set_fir_en(false)?;
                        self.fir_dec = None;
                    }
                }
            }
        }
        if cur
            .as_ref()
            .is_none_or(|c| c.bw_hz != b.bw_hz || rate_changed)
        {
            self.phy_rx.set_attr("rf_bandwidth", &b.bw_hz.to_string())?;
        }
        if cur.as_ref().is_none_or(|c| c.centre_hz != b.centre_hz) {
            self.rx_lo.set_attr("frequency", &b.centre_hz.to_string())?;
        }
        if cur.as_ref().is_none_or(|c| c.gain != b.gain) {
            match b.gain {
                Gain::Manual(g) => {
                    self.phy_rx.set_attr("gain_control_mode", "manual")?;
                    self.phy_rx.set_attr("hardwaregain", &format!("{g:.2}"))?;
                }
                Gain::Auto => {
                    let mode = self.phy_rx.attr("gain_control_mode").ok();
                    Self::set_if_changed(
                        &self.phy_rx,
                        "gain_control_mode",
                        "slow_attack",
                        mode.as_deref(),
                    )?;
                }
            }
        }
        self.cur = Some(b.clone());
        self.rx_i.enable();
        self.rx_q.enable();
        let mut buf = self.rx.buffer(self.block)?;
        // The first buffer after a retune holds the old PLL's tail: read it and drop it.
        buf.refill()?;
        self.buf = Some(buf);
        Ok(())
    }

    fn refill(&mut self, out: &mut Vec<u8>) -> Result<(), RefillError> {
        let want = self.block * 4;
        let buf = self
            .buf
            .as_mut()
            .ok_or_else(|| RefillError::Other("not tuned".into()))?;
        let n = buf.refill().map_err(RefillError::Lost)?;
        let d = buf.data(n);
        if d.len() < want {
            return Err(RefillError::Other(format!(
                "short buffer: {} of {want} bytes",
                d.len()
            )));
        }
        out.clear();
        out.extend_from_slice(&d[..want]);
        Ok(())
    }

    fn safe_state(&self) -> SafeState {
        self.safe.clone()
    }

    fn hw_status(&mut self) -> HwStatus {
        let rd = |c: &Channel, a: &str| c.attr(a).ok();
        let zynq = self.xadc_temp.and_then(|c| {
            let raw = num(&c.attr("raw").ok()?)?;
            let off = c
                .attr("offset")
                .ok()
                .as_deref()
                .and_then(num)
                .unwrap_or(0.0);
            let scale = c.attr("scale").ok().as_deref().and_then(num).unwrap_or(1.0);
            plausible_temp((raw + off) * scale / 1000.0)
        });
        HwStatus {
            lo_hz: rd(&self.rx_lo, "frequency").and_then(|s| s.parse().ok()),
            rate_sps: rd(&self.phy_rx, "sampling_frequency").and_then(|s| s.parse().ok()),
            bw_hz: rd(&self.phy_rx, "rf_bandwidth").and_then(|s| s.parse().ok()),
            gain_db: rd(&self.phy_rx, "hardwaregain").as_deref().and_then(num),
            gain_mode: rd(&self.phy_rx, "gain_control_mode"),
            rssi_db: rd(&self.phy_rx, "rssi").as_deref().and_then(num),
            temp_phy_c: self
                .temp
                .and_then(|c| c.attr("input").ok()?.parse::<f64>().ok())
                .and_then(|m| plausible_temp(m / 1000.0)),
            temp_zynq_c: zynq,
        }
    }
}

/// Measure what a backend delivers: open it, run the receive path above the USB 2.0 ceiling
/// for about a second and divide. Receive only; the tuning is left at 2437 MHz.
pub fn measure(uri: &str) -> Result<f64, String> {
    let mut p = Pluto::open(uri, 65_536)?;
    let mut b = Band::new(2_437_000_000, 10_000_000, 1000);
    b.gain = Gain::Manual(40.0);
    p.apply(&b)?;
    let mut out = Vec::new();
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    while t0.elapsed() < std::time::Duration::from_millis(1500) {
        match p.refill(&mut out) {
            Ok(()) => n += 65_536,
            Err(RefillError::Lost(e)) | Err(RefillError::Other(e)) => return Err(e),
            Err(RefillError::Eof) => break,
        }
    }
    Ok(n as f64 / t0.elapsed().as_secs_f64() / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_rates_pick_a_decimation() {
        assert_eq!(fir_for(5_000_000), None);
        assert_eq!(fir_for(2_083_334), None);
        assert_eq!(fir_for(2_000_000), Some(2));
        assert_eq!(fir_for(1_041_667), Some(2));
        assert_eq!(fir_for(1_000_000), Some(4));
        assert_eq!(fir_for(520_833), Some(4));
    }

    #[test]
    fn the_fir_text_is_what_the_driver_parses() {
        for dec in [2, 4] {
            let t = fir_config(dec);
            let mut lines = t.lines();
            assert_eq!(
                lines.next(),
                Some(format!("RX 3 GAIN 0 DEC {dec}").as_str())
            );
            assert_eq!(
                lines.next(),
                Some(format!("TX 3 GAIN -6 INT {dec}").as_str())
            );
            let taps: Vec<(i32, i32)> = lines
                .map(|l| {
                    let (a, b) = l.split_once(',').unwrap();
                    (a.parse().unwrap(), b.parse().unwrap())
                })
                .collect();
            assert_eq!(taps.len(), 64);
            assert!(taps.iter().all(|(a, b)| a == b && a.abs() <= 32767));
            // symmetric lowpass, unity gain just under full scale
            for i in 0..32 {
                assert!((taps[i].0 - taps[63 - i].0).abs() <= 1);
            }
            let sum: i32 = taps.iter().map(|t| t.0).sum();
            assert!((29_000..=30_000).contains(&sum), "dc gain {sum}");
        }
    }

    #[test]
    fn implausible_temperatures_are_dropped() {
        assert_eq!(plausible_temp(41.2), Some(41.2));
        assert_eq!(plausible_temp(-80.702), None);
        assert_eq!(plausible_temp(-75.439), None);
        assert_eq!(plausible_temp(300.0), None);
    }

    #[test]
    fn attribute_numbers_parse_with_their_units() {
        assert_eq!(num("40.000000 dB"), Some(40.0));
        assert_eq!(num("-89.750000 dB"), Some(-89.75));
        assert_eq!(num("92.75 dB"), Some(92.75));
        assert_eq!(num(""), None);
    }
}
