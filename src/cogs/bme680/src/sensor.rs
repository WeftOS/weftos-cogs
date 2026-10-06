//! Linux i2c-dev access for one forced-mode TPHG sample.
//!
//! The only writes are the datasheet quick-start sequence (BST-BME680-DS001-09
//! §3.2.1): `ctrl_hum`, `ctrl_meas` (sleep, so oversampling is applied before
//! the mode change), `ctrl_gas_0` (`heat_off` clear), `res_heat_0`, `gas_wait_0`,
//! `ctrl_gas_1`, then `ctrl_meas` with mode `0b01`. The part returns to sleep
//! by itself (§3.1). Register `0xE0` (soft reset) is never written. The I2C
//! address is never changed.
//!
//! I2C reads use a repeated start (`I2C_RDWR`), which §6.2.2 allows. Writes are
//! one address/data pair each, because a multi-byte write is not auto-incremented
//! (§6.2.1).

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::thread;
use std::time::{Duration, Instant};

use crate::compensate::{
    self, ctrl_meas, decode_calib, decode_field, gas_wait_code, res_heat_code, tphg_wait_us, Calib,
    Reading, CHIP_ID, LEN_COEFF1, LEN_COEFF2, LEN_COEFF3, LEN_FIELD, REG_CHIP_ID, REG_COEFF1,
    REG_COEFF2, REG_COEFF3, REG_CTRL_GAS_0, REG_CTRL_GAS_1, REG_CTRL_HUM, REG_CTRL_MEAS, REG_FIELD,
    REG_GAS_WAIT0, REG_RES_HEAT0,
};

const I2C_RDWR: libc::c_ulong = 0x0707;
const I2C_M_RD: u16 = 0x0001;

#[repr(C)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
struct I2cRdwr {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

#[derive(Debug)]
pub struct Bme680 {
    dev: File,
    path: String,
    addr: u8,
    cal: Calib,
    /// Ambient temperature for the next heater code. Starts at 25 °C, which
    /// §3.3.5 allows to be hardcoded until a sample exists.
    amb_c: i32,
}

pub fn check_addr(addr: u8) -> Result<(), String> {
    if addr == 0x76 || addr == 0x77 {
        Ok(())
    } else {
        Err(format!(
            "address 0x{addr:02x} is not a BME680 I2C address (0x76 if SDO is GND, 0x77 if SDO is VDDIO)"
        ))
    }
}

fn hint(path: &str, addr: u8, detail: &str) -> String {
    format!(
        "{detail} at 0x{addr:02x} on {path} (check SDA to header pin 3, SCL to pin 5, CSB tied to 3.3 V so the part stays in I2C, SDO strapped for this address, and power-cycle the breakout; a Seed reboot does not drop 3V3)"
    )
}

impl Bme680 {
    pub fn open(path: &str, addr: u8) -> Result<Self, String> {
        check_addr(addr)?;
        let dev = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| hint(path, addr, &format!("open {path}: {e}; nothing answered")))?;
        let mut s = Self {
            dev,
            path: path.to_string(),
            addr,
            cal: decode_calib(&[0; LEN_COEFF1], &[0; LEN_COEFF2], &[0; LEN_COEFF3])
                .expect("zero calibration blocks have the right lengths"),
            amb_c: 25,
        };
        let mut id = [0u8; 1];
        s.read_regs(REG_CHIP_ID, &mut id)
            .map_err(|e| hint(path, addr, &format!("nothing answered ({e})")))?;
        if id[0] != CHIP_ID {
            return Err(format!(
                "chip id 0x{:02x} at 0x{addr:02x} on {path} is not the BME680 id 0x61 (BST-BME680-DS001-09 §5.3.1.6); not reporting a sample",
                id[0]
            ));
        }
        let mut c1 = [0u8; LEN_COEFF1];
        let mut c2 = [0u8; LEN_COEFF2];
        let mut c3 = [0u8; LEN_COEFF3];
        s.read_regs(REG_COEFF1, &mut c1)
            .map_err(|e| hint(path, addr, &e))?;
        s.read_regs(REG_COEFF2, &mut c2)
            .map_err(|e| hint(path, addr, &e))?;
        s.read_regs(REG_COEFF3, &mut c3)
            .map_err(|e| hint(path, addr, &e))?;
        s.cal = decode_calib(&c1, &c2, &c3)?;
        Ok(s)
    }

    fn rdwr(&mut self, msgs: &mut [I2cMsg]) -> Result<(), String> {
        let mut data = I2cRdwr {
            msgs: msgs.as_mut_ptr(),
            nmsgs: u32::try_from(msgs.len()).unwrap_or(0),
        };
        // SAFETY: `msgs` and their buffers live for this call. The ioctl reads
        // or writes only `len` bytes at each `buf`. The fd belongs to `self`.
        let rc = unsafe { libc::ioctl(self.dev.as_raw_fd(), I2C_RDWR, &mut data) };
        if rc < 0 {
            Err(format!("i2c: {}", std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    fn write_reg(&mut self, reg: u8, val: u8) -> Result<(), String> {
        let mut buf = [reg, val];
        let mut msgs = [I2cMsg {
            addr: u16::from(self.addr),
            flags: 0,
            len: 2,
            buf: buf.as_mut_ptr(),
        }];
        self.rdwr(&mut msgs)
            .map_err(|e| format!("write 0x{reg:02x}: {e}"))
    }

    fn read_regs(&mut self, reg: u8, out: &mut [u8]) -> Result<(), String> {
        if out.is_empty() || out.len() > 32 {
            return Err(format!("refusing a {}-byte I2C read", out.len()));
        }
        let mut reg_buf = [reg];
        let mut msgs = [
            I2cMsg {
                addr: u16::from(self.addr),
                flags: 0,
                len: 1,
                buf: reg_buf.as_mut_ptr(),
            },
            I2cMsg {
                addr: u16::from(self.addr),
                flags: I2C_M_RD,
                len: u16::try_from(out.len()).unwrap_or(0),
                buf: out.as_mut_ptr(),
            },
        ];
        self.rdwr(&mut msgs)
            .map_err(|e| format!("read 0x{reg:02x}: {e}"))
    }

    /// One forced-mode sample. On failure the error names the bus and address
    /// and no compensated numbers are returned.
    pub fn sample(&mut self, heater_c: u16, heat_ms: u16) -> Result<Reading, String> {
        let heat = gas_wait_code(heat_ms);
        let heater = res_heat_code(&self.cal, heater_c, self.amb_c);
        // Humidity oversampling is latched by the following ctrl_meas write (§3.2.1).
        self.write_reg(REG_CTRL_HUM, compensate::OSRS_H_1X)
            .map_err(|e| hint(&self.path, self.addr, &e))?;
        self.write_reg(REG_CTRL_MEAS, ctrl_meas(0b00))
            .map_err(|e| hint(&self.path, self.addr, &e))?;
        self.write_reg(REG_CTRL_GAS_0, 0x00)
            .map_err(|e| hint(&self.path, self.addr, &e))?;
        let run_gas = match heater {
            Ok(code) => {
                self.write_reg(REG_RES_HEAT0, code)
                    .map_err(|e| hint(&self.path, self.addr, &e))?;
                self.write_reg(REG_GAS_WAIT0, heat)
                    .map_err(|e| hint(&self.path, self.addr, &e))?;
                true
            }
            Err(_) => false,
        };
        // run_gas at bit 4, nb_conv = 0 (§5.3.3.5, §5.3.3.6).
        self.write_reg(REG_CTRL_GAS_1, if run_gas { 0x10 } else { 0x00 })
            .map_err(|e| hint(&self.path, self.addr, &e))?;
        self.write_reg(REG_CTRL_MEAS, ctrl_meas(0b01))
            .map_err(|e| hint(&self.path, self.addr, &e))?;

        let wait = Duration::from_micros(tphg_wait_us(heat_ms));
        thread::sleep(wait);
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let mut status = [0u8; 1];
            self.read_regs(REG_FIELD, &mut status)
                .map_err(|e| hint(&self.path, self.addr, &e))?;
            if status[0] & 0x20 == 0 {
                break;
            }
            if Instant::now() > deadline {
                return Err(hint(
                    &self.path,
                    self.addr,
                    "measurement did not finish (measuring bit stayed set)",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut field_raw = [0u8; LEN_FIELD];
        self.read_regs(REG_FIELD, &mut field_raw)
            .map_err(|e| hint(&self.path, self.addr, &e))?;
        let field = decode_field(&field_raw)?;
        if !field.new_data {
            return Err(hint(
                &self.path,
                self.addr,
                "no new data after forced mode (new_data_0 clear)",
            ));
        }
        let mut reading = compensate::compensate(&self.cal, &field);
        if let Err(why) = heater {
            reading.gas_ohm = None;
            reading.gaps.push(format!("heater code not written: {why}"));
        }
        if let Some(t) = reading.temperature_c {
            if (-40.0..=85.0).contains(&t) {
                self.amb_c = t.round() as i32;
            }
        }
        Ok(reading)
    }
}

/// `--simulate`: the fixed image goes through the same decoder as a real field.
pub fn simulate() -> Result<Reading, String> {
    compensate::reading_from_image(&compensate::synthetic_image())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_address_is_refused_before_opening_the_bus() {
        let err = Bme680::open("/dev/i2c-1", 0x00).unwrap_err();
        assert!(err.contains("not a BME680"), "{err}");
        assert!(err.contains("0x00"), "{err}");
    }

    #[test]
    fn missing_device_names_the_bus_and_address() {
        let err = Bme680::open("/dev/bme680-test-does-not-exist", 0x76).unwrap_err();
        assert!(err.contains("/dev/bme680-test-does-not-exist"), "{err}");
        assert!(err.contains("0x76"), "{err}");
        assert!(err.contains("3V3") || err.contains("3.3"), "{err}");
    }

    #[test]
    fn simulate_is_the_decoder_on_the_synthetic_image() {
        let got = simulate().unwrap();
        let expect = compensate::reading_from_image(&compensate::synthetic_image()).unwrap();
        assert_eq!(got, expect);
        assert!(got.all_present());
    }
}
