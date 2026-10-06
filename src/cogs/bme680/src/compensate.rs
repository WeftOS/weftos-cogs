//! BME680 register decode and compensation.
//!
//! Formulas: Bosch Sensortec BST-BME680-DS001-09, revision 1.9, February 2024,
//! sections 3.3.1 (temperature), 3.3.2 (pressure), 3.3.3 (humidity), 3.3.5
//! (heater code) and 3.4.1 (gas resistance). Floating-point forms. The integer
//! forms in the same sections are not used for the published values.
//!
//! Signedness and the calibration byte indexes follow Bosch's BME68x Sensor API
//! v4.4.8 (`get_calib_data`), commit 80ea120a8b8ac987d7d79eb68a9ed796736be845,
//! because the datasheet tables give addresses but not every signedness. The
//! datasheet's own gas-resistance constants (Table 16) are used. The API's
//! float gas path (`calc_gas_resistance_low` / `_high`) does not match §3.4.1
//! and is not used.
//!
//! A skipped or power-on temperature/pressure ADC is `0x80000` (§3.3.4; MSB
//! reset state `0x80` in Table 20). Humidity's skipped value is `0x8000`
//! (§5.3.2.1). Those are not readings. Gas is published only when `gas_valid_r`
//! and `heat_stab_r` are both set (§3.4, §5.3.5.5, §5.3.5.6).

pub const CHIP_ID: u8 = 0x61;
pub const REG_CHIP_ID: u8 = 0xD0;
pub const REG_COEFF1: u8 = 0x8A;
pub const REG_COEFF2: u8 = 0xE1;
pub const REG_COEFF3: u8 = 0x00;
pub const LEN_COEFF1: usize = 23;
pub const LEN_COEFF2: usize = 14;
pub const LEN_COEFF3: usize = 5;
pub const REG_FIELD: u8 = 0x1D;
pub const LEN_FIELD: usize = 15;

pub const REG_CTRL_HUM: u8 = 0x72;
pub const REG_CTRL_MEAS: u8 = 0x74;
pub const REG_CTRL_GAS_1: u8 = 0x71;
pub const REG_CTRL_GAS_0: u8 = 0x70;
pub const REG_GAS_WAIT0: u8 = 0x64;
pub const REG_RES_HEAT0: u8 = 0x5A;

/// §5.3.2 quick-start oversampling: humidity ×1, temperature ×2, pressure ×16.
pub const OSRS_H_1X: u8 = 0b001;
pub const OSRS_T_2X: u8 = 0b010;
pub const OSRS_P_16X: u8 = 0b101;

const TEMP_SKIPPED: u32 = 0x8_0000;
const PRESS_SKIPPED: u32 = 0x8_0000;
const HUM_SKIPPED: u16 = 0x8000;

const GAS_CONST1: [f64; 16] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 0.99, 1.0, 0.992, 1.0, 1.0, 0.998, 0.995, 1.0, 0.99, 1.0, 1.0,
];
const GAS_CONST2: [f64; 16] = [
    8_000_000.0,
    4_000_000.0,
    2_000_000.0,
    1_000_000.0,
    499_500.499_5,
    248_262.164_8,
    125_000.0,
    63_004.032_26,
    31_281.281_28,
    15_625.0,
    7_812.5,
    3_906.25,
    1_953.125,
    976.5625,
    488.28125,
    244.140625,
];

#[derive(Clone, Debug, PartialEq)]
pub struct Calib {
    pub par_t1: u16,
    pub par_t2: i16,
    pub par_t3: i8,
    pub par_p1: u16,
    pub par_p2: i16,
    pub par_p3: i8,
    pub par_p4: i16,
    pub par_p5: i16,
    pub par_p6: i8,
    pub par_p7: i8,
    pub par_p8: i16,
    pub par_p9: i16,
    pub par_p10: u8,
    pub par_h1: u16,
    pub par_h2: u16,
    pub par_h3: i8,
    pub par_h4: i8,
    pub par_h5: i8,
    pub par_h6: u8,
    pub par_h7: i8,
    pub par_g1: i8,
    pub par_g2: i16,
    pub par_g3: i8,
    pub res_heat_range: u8,
    pub res_heat_val: i8,
    pub range_sw_err: i8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub new_data: bool,
    pub gas_measuring: bool,
    pub measuring: bool,
    pub temp_adc: u32,
    pub press_adc: u32,
    pub hum_adc: u16,
    pub gas_adc: u16,
    pub gas_range: u8,
    pub gas_valid: bool,
    pub heat_stable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub temperature_c: Option<f64>,
    pub pressure_pa: Option<f64>,
    pub humidity_rh: Option<f64>,
    pub gas_ohm: Option<f64>,
    /// Why a quantity is absent. Empty when all four are present.
    pub gaps: Vec<String>,
}

impl Reading {
    pub fn all_present(&self) -> bool {
        self.temperature_c.is_some()
            && self.pressure_pa.is_some()
            && self.humidity_rh.is_some()
            && self.gas_ohm.is_some()
    }
}

/// `ctrl_meas` with the quick-start oversampling and `mode` in bits 1:0.
/// Sleep is `0b00`, forced is `0b01` (§3.1, §5.3.1.3, §5.3.2).
pub fn ctrl_meas(mode: u8) -> u8 {
    (OSRS_T_2X << 5) | (OSRS_P_16X << 2) | (mode & 0b11)
}

/// `gas_wait_x` for a duration in milliseconds. §5.3.3.3: bits 5:0 are a 0–63
/// count, bits 7:6 multiply by 1, 4, 16 or 64. The datasheet's 100 ms example
/// is `0x59`. Durations above 4032 ms do not fit; the caller bounds them.
pub fn gas_wait_code(mut dur_ms: u16) -> u8 {
    let mut factor: u16 = 0;
    while dur_ms > 0x3F && factor < 3 {
        dur_ms /= 4;
        factor += 1;
    }
    // 63 × 64 = 4032 ms is the longest code. Longer requests use that maximum
    // rather than wrapping the register byte.
    if dur_ms > 0x3F {
        return 0xFF;
    }
    (dur_ms + factor * 64) as u8
}

/// Heater register code for `target_c`, §3.3.5 floating-point form.
/// `amb_c` is the ambient temperature the datasheet allows to be hardcoded or
/// taken from the temperature sensor. Refuses a code outside 0..=255 rather
/// than truncating into a different target.
pub fn res_heat_code(cal: &Calib, target_c: u16, amb_c: i32) -> Result<u8, &'static str> {
    let target = f64::from(target_c);
    let amb = f64::from(amb_c);
    let var1 = (f64::from(cal.par_g1) / 16.0) + 49.0;
    let var2 = ((f64::from(cal.par_g2) / 32768.0) * 0.0005) + 0.00235;
    let var3 = f64::from(cal.par_g3) / 1024.0;
    let var4 = var1 * (1.0 + (var2 * target));
    let var5 = var4 + (var3 * amb);
    let range = f64::from(cal.res_heat_range);
    let heat_val = f64::from(cal.res_heat_val);
    let denom = 1.0 + (heat_val * 0.002);
    if denom == 0.0 || (4.0 + range) == 0.0 {
        return Err("heater code denominator is zero");
    }
    let code = 3.4 * ((var5 * (4.0 / (4.0 + range)) * (1.0 / denom)) - 25.0);
    if !code.is_finite() || !(0.0..256.0).contains(&code) {
        return Err("heater code is outside 0..=255");
    }
    Ok(code as u8)
}

/// TPH measurement wait from Bosch BME68x Sensor API v4.4.8 `bme68x_get_meas_dur`
/// for oversampling ×2 / ×16 / ×1, plus the programmed heater time.
/// The datasheet defines the `measuring` bit (§5.3.5.3) but not this microsecond
/// figure; it is only a wait hint, and the bit is what decides completion.
pub fn tphg_wait_us(heat_ms: u16) -> u64 {
    let cycles: u64 = 2 + 16 + 1;
    cycles * 1963 + 477 * 4 + 477 * 5 + 1000 + u64::from(heat_ms) * 1000
}

pub fn decode_calib(c1: &[u8], c2: &[u8], c3: &[u8]) -> Result<Calib, String> {
    if c1.len() != LEN_COEFF1 || c2.len() != LEN_COEFF2 || c3.len() != LEN_COEFF3 {
        return Err(format!(
            "calibration block lengths {}/{}/{} are not {}/{}/{}",
            c1.len(),
            c2.len(),
            c3.len(),
            LEN_COEFF1,
            LEN_COEFF2,
            LEN_COEFF3
        ));
    }
    // Indexes are into the concatenation the API uses: 0x8A.., then 0xE1.., then 0x00..
    // c1 is bytes 0..22, c2 is 23..36, c3 is 37..41.
    let b = |i: usize| -> u8 {
        if i < LEN_COEFF1 {
            c1[i]
        } else if i < LEN_COEFF1 + LEN_COEFF2 {
            c2[i - LEN_COEFF1]
        } else {
            c3[i - LEN_COEFF1 - LEN_COEFF2]
        }
    };
    Ok(Calib {
        par_t1: u16::from_le_bytes([b(31), b(32)]),
        par_t2: i16::from_le_bytes([b(0), b(1)]),
        par_t3: b(2) as i8,
        par_p1: u16::from_le_bytes([b(4), b(5)]),
        par_p2: i16::from_le_bytes([b(6), b(7)]),
        par_p3: b(8) as i8,
        par_p4: i16::from_le_bytes([b(10), b(11)]),
        par_p5: i16::from_le_bytes([b(12), b(13)]),
        par_p6: b(15) as i8,
        par_p7: b(14) as i8,
        par_p8: i16::from_le_bytes([b(18), b(19)]),
        par_p9: i16::from_le_bytes([b(20), b(21)]),
        par_p10: b(22),
        par_h1: (u16::from(b(25)) << 4) | u16::from(b(24) & 0x0F),
        par_h2: (u16::from(b(23)) << 4) | u16::from(b(24) >> 4),
        par_h3: b(26) as i8,
        par_h4: b(27) as i8,
        par_h5: b(28) as i8,
        par_h6: b(29),
        par_h7: b(30) as i8,
        par_g1: b(35) as i8,
        par_g2: i16::from_le_bytes([b(33), b(34)]),
        par_g3: b(36) as i8,
        res_heat_range: (b(39) & 0x30) >> 4,
        res_heat_val: b(37) as i8,
        range_sw_err: ((b(41) & 0xF0) as i8) / 16,
    })
}

pub fn decode_field(raw: &[u8]) -> Result<Field, String> {
    if raw.len() != LEN_FIELD {
        return Err(format!(
            "field length {} is not {LEN_FIELD} (registers 0x1D..=0x2B)",
            raw.len()
        ));
    }
    let status = raw[0];
    let press = (u32::from(raw[2]) << 12) | (u32::from(raw[3]) << 4) | (u32::from(raw[4]) >> 4);
    let temp = (u32::from(raw[5]) << 12) | (u32::from(raw[6]) << 4) | (u32::from(raw[7]) >> 4);
    let hum = (u16::from(raw[8]) << 8) | u16::from(raw[9]);
    let gas_lsb = raw[14];
    let gas_adc = (u16::from(raw[13]) << 2) | u16::from(gas_lsb >> 6);
    Ok(Field {
        new_data: status & 0x80 != 0,
        gas_measuring: status & 0x40 != 0,
        measuring: status & 0x20 != 0,
        temp_adc: temp,
        press_adc: press,
        hum_adc: hum,
        gas_adc,
        gas_range: gas_lsb & 0x0F,
        gas_valid: gas_lsb & 0x20 != 0,
        heat_stable: gas_lsb & 0x10 != 0,
    })
}

fn finite(v: f64) -> Option<f64> {
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

/// §3.3.1. Returns `(t_fine, temp_comp)`.
fn compensate_temperature(cal: &Calib, temp_adc: u32) -> (f64, f64) {
    let adc = f64::from(temp_adc);
    let t1 = f64::from(cal.par_t1);
    let t2 = f64::from(cal.par_t2);
    let t3 = f64::from(cal.par_t3);
    let var1 = ((adc / 16384.0) - (t1 / 1024.0)) * t2;
    let diff = (adc / 131072.0) - (t1 / 8192.0);
    let var2 = (diff * diff) * (t3 * 16.0);
    let t_fine = var1 + var2;
    (t_fine, t_fine / 5120.0)
}

/// §3.3.2. `None` when the `par_p1` term is zero (the formula would divide by it).
fn compensate_pressure(cal: &Calib, press_adc: u32, t_fine: f64) -> Option<f64> {
    let p1 = f64::from(cal.par_p1);
    let p2 = f64::from(cal.par_p2);
    let p3 = f64::from(cal.par_p3);
    let p4 = f64::from(cal.par_p4);
    let p5 = f64::from(cal.par_p5);
    let p6 = f64::from(cal.par_p6);
    let p7 = f64::from(cal.par_p7);
    let p8 = f64::from(cal.par_p8);
    let p9 = f64::from(cal.par_p9);
    let p10 = f64::from(cal.par_p10);
    let mut var1 = (t_fine / 2.0) - 64000.0;
    let mut var2 = var1 * var1 * (p6 / 131072.0);
    var2 += var1 * p5 * 2.0;
    var2 = (var2 / 4.0) + (p4 * 65536.0);
    var1 = (((p3 * var1 * var1) / 16384.0) + (p2 * var1)) / 524288.0;
    var1 = (1.0 + (var1 / 32768.0)) * p1;
    if var1 == 0.0 {
        return None;
    }
    let mut press = 1_048_576.0 - f64::from(press_adc);
    press = ((press - (var2 / 4096.0)) * 6250.0) / var1;
    var1 = (p9 * press * press) / 2_147_483_648.0;
    var2 = press * (p8 / 32768.0);
    let var3 = (press / 256.0) * (press / 256.0) * (press / 256.0) * (p10 / 131072.0);
    press += (var1 + var2 + var3 + (p7 * 128.0)) / 16.0;
    finite(press)
}

/// §3.3.3. Not clamped to 0..=100. The BME68x API float path clamps; the
/// datasheet formula does not, and a clamp would hide a bad coefficient.
fn compensate_humidity(cal: &Calib, hum_adc: u16, temp_comp: f64) -> Option<f64> {
    let h1 = f64::from(cal.par_h1);
    let h2 = f64::from(cal.par_h2);
    let h3 = f64::from(cal.par_h3);
    let h4 = f64::from(cal.par_h4);
    let h5 = f64::from(cal.par_h5);
    let h6 = f64::from(cal.par_h6);
    let h7 = f64::from(cal.par_h7);
    let var1 = f64::from(hum_adc) - ((h1 * 16.0) + ((h3 / 2.0) * temp_comp));
    let var2 = var1
        * ((h2 / 262144.0)
            * (1.0 + ((h4 / 16384.0) * temp_comp) + ((h5 / 1_048_576.0) * temp_comp * temp_comp)));
    let var3 = h6 / 16384.0;
    let var4 = h7 / 2_097_152.0;
    finite(var2 + ((var3 + (var4 * temp_comp)) * var2 * var2))
}

/// §3.4.1. `None` when the denominator is zero or the result is not a positive
/// finite resistance. A zero is not a reading.
fn compensate_gas(cal: &Calib, gas_adc: u16, gas_range: u8) -> Option<f64> {
    let range = usize::from(gas_range);
    if range >= GAS_CONST1.len() {
        return None;
    }
    let var1 = (1340.0 + 5.0 * f64::from(cal.range_sw_err)) * GAS_CONST1[range];
    let denom = f64::from(gas_adc) - 512.0 + var1;
    if denom == 0.0 {
        return None;
    }
    let gas = var1 * GAS_CONST2[range] / denom;
    finite(gas).filter(|g| *g > 0.0)
}

pub fn compensate(cal: &Calib, field: &Field) -> Reading {
    let mut gaps = Vec::new();
    let (t_fine, temp) = if field.temp_adc == TEMP_SKIPPED {
        gaps.push("temperature ADC is the skipped/reset value 0x80000".to_string());
        (None, None)
    } else {
        let (tf, t) = compensate_temperature(cal, field.temp_adc);
        match finite(t) {
            Some(t) => (Some(tf), Some(t)),
            None => {
                gaps.push("temperature compensation was not finite".to_string());
                (None, None)
            }
        }
    };

    let pressure = if field.press_adc == PRESS_SKIPPED {
        gaps.push("pressure ADC is the skipped/reset value 0x80000".to_string());
        None
    } else if let Some(tf) = t_fine {
        match compensate_pressure(cal, field.press_adc, tf) {
            Some(p) => Some(p),
            None => {
                gaps.push("pressure compensation was not finite or divided by zero".to_string());
                None
            }
        }
    } else {
        gaps.push("pressure needs a compensated temperature".to_string());
        None
    };

    let humidity = if field.hum_adc == HUM_SKIPPED {
        gaps.push("humidity ADC is the skipped/reset value 0x8000".to_string());
        None
    } else if let Some(t) = temp {
        match compensate_humidity(cal, field.hum_adc, t) {
            Some(h) => Some(h),
            None => {
                gaps.push("humidity compensation was not finite".to_string());
                None
            }
        }
    } else {
        gaps.push("humidity needs a compensated temperature".to_string());
        None
    };

    let gas = if !field.gas_valid {
        gaps.push("gas_valid_r is clear; gas resistance is not a completed conversion".to_string());
        None
    } else if !field.heat_stable {
        gaps.push("heat_stab_r is clear; the heater did not reach the target".to_string());
        None
    } else {
        match compensate_gas(cal, field.gas_adc, field.gas_range) {
            Some(g) => Some(g),
            None => {
                gaps.push(
                    "gas resistance compensation was not a positive finite value".to_string(),
                );
                None
            }
        }
    };

    Reading {
        temperature_c: temp,
        pressure_pa: pressure,
        humidity_rh: humidity,
        gas_ohm: gas,
        gaps,
    }
}

/// Fixed calibration image and ADC field used by `--simulate` and the tests.
/// The bytes are the input; compensation is the same function as a real sample.
pub struct Image {
    pub coeff1: [u8; LEN_COEFF1],
    pub coeff2: [u8; LEN_COEFF2],
    pub coeff3: [u8; LEN_COEFF3],
    pub field: [u8; LEN_FIELD],
}

pub fn synthetic_image() -> Image {
    // Coefficients (see tests for the decoded values):
    // par_t2 = 26372, par_t3 = 3, par_p1 = 36477, par_p2 = -10612, par_p3 = 88,
    // par_p4 = 7500, par_p5 = -145, par_p7 = -12, par_p6 = 30, par_p8 = -800,
    // par_p9 = -3277, par_p10 = 30.
    let mut coeff1 = [0u8; LEN_COEFF1];
    coeff1[0] = 0x04;
    coeff1[1] = 0x67;
    coeff1[2] = 3;
    coeff1[4] = 0x7D;
    coeff1[5] = 0x8E;
    coeff1[6] = 0x8C;
    coeff1[7] = 0xD6;
    coeff1[8] = 88;
    coeff1[10] = 0x4C;
    coeff1[11] = 0x1D;
    coeff1[12] = 0x6F;
    coeff1[13] = 0xFF;
    coeff1[14] = 0xF4;
    coeff1[15] = 30;
    coeff1[18] = 0xE0;
    coeff1[19] = 0xFC;
    coeff1[20] = 0x33;
    coeff1[21] = 0xF3;
    coeff1[22] = 30;
    // par_h2 = 1024, par_h1 = 700, par_h3 = 0, par_h4 = 45, par_h5 = 20,
    // par_h6 = 120, par_h7 = -100, par_t1 = 26214, par_g2 = -5000, par_g1 = 100, par_g3 = 18.
    let mut coeff2 = [0u8; LEN_COEFF2];
    coeff2[0] = 0x40;
    coeff2[1] = 0x0C;
    coeff2[2] = 0x2B;
    coeff2[4] = 45;
    coeff2[5] = 20;
    coeff2[6] = 120;
    coeff2[7] = 0x9C;
    coeff2[8] = 0x66;
    coeff2[9] = 0x66;
    coeff2[10] = 0x78;
    coeff2[11] = 0xEC;
    coeff2[12] = 100;
    coeff2[13] = 18;
    // res_heat_val = 40, res_heat_range = 1 (bits 5:4), range_sw_err = -1 (bits 7:4).
    let mut coeff3 = [0u8; LEN_COEFF3];
    coeff3[0] = 40;
    coeff3[2] = 0x10;
    coeff3[4] = 0xF0;
    // new_data set. press_adc 375000, temp_adc 519888, hum_adc 25000,
    // gas_adc 400, gas_valid, heat_stable, gas_range 4.
    let field = [
        0x80, 0x00, 0x5B, 0x8D, 0x80, 0x7E, 0xED, 0x00, 0x61, 0xA8, 0x00, 0x00, 0x00, 0x64, 0x34,
    ];
    Image {
        coeff1,
        coeff2,
        coeff3,
        field,
    }
}

pub fn reading_from_image(image: &Image) -> Result<Reading, String> {
    let cal = decode_calib(&image.coeff1, &image.coeff2, &image.coeff3)?;
    let field = decode_field(&image.field)?;
    if !field.new_data {
        return Err(
            "new_data_0 is clear; refusing to compensate a field that is not a new sample"
                .to_string(),
        );
    }
    Ok(compensate(&cal, &field))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_reading() -> Reading {
        reading_from_image(&synthetic_image()).unwrap()
    }

    #[test]
    fn datasheet_gas_wait_example_is_0x59() {
        // BST-BME680-DS001-09 §3.2.1 step 4: 100 ms is gas_wait 0x59.
        assert_eq!(gas_wait_code(100), 0x59);
        assert_eq!(gas_wait_code(1), 1);
        assert_eq!(gas_wait_code(4032), 0xFF);
        assert_eq!(gas_wait_code(0), 0);
    }

    #[test]
    fn quick_start_ctrl_meas_sleep_then_forced() {
        assert_eq!(ctrl_meas(0b00), 0x54);
        assert_eq!(ctrl_meas(0b01), 0x55);
    }

    #[test]
    fn nibble_fields_match_the_api_shift() {
        assert_eq!(((0xF0u8 & 0xF0) as i8) / 16, -1);
        assert_eq!(((0x70u8 & 0xF0) as i8) / 16, 7);
        assert_eq!(((0x80u8 & 0xF0) as i8) / 16, -8);
        // res_heat_range is bits 5:4. 0x10 is 1 (the synthetic image). 0x20 is 2.
        assert_eq!((0x10u8 & 0x30) >> 4, 1);
        assert_eq!((0x20u8 & 0x30) >> 4, 2);
    }

    #[test]
    fn synthetic_bytes_decode_to_the_chosen_coefficients_and_adcs() {
        let image = synthetic_image();
        let cal = decode_calib(&image.coeff1, &image.coeff2, &image.coeff3).unwrap();
        assert_eq!(cal.par_t1, 26214);
        assert_eq!(cal.par_t2, 26372);
        assert_eq!(cal.par_t3, 3);
        assert_eq!(cal.par_p1, 36477);
        assert_eq!(cal.par_p2, -10612);
        assert_eq!(cal.par_p3, 88);
        assert_eq!(cal.par_p4, 7500);
        assert_eq!(cal.par_p5, -145);
        assert_eq!(cal.par_p6, 30);
        assert_eq!(cal.par_p7, -12);
        assert_eq!(cal.par_p8, -800);
        assert_eq!(cal.par_p9, -3277);
        assert_eq!(cal.par_p10, 30);
        assert_eq!(cal.par_h1, 700);
        assert_eq!(cal.par_h2, 1024);
        assert_eq!(cal.par_h3, 0);
        assert_eq!(cal.par_h4, 45);
        assert_eq!(cal.par_h5, 20);
        assert_eq!(cal.par_h6, 120);
        assert_eq!(cal.par_h7, -100);
        assert_eq!(cal.par_g1, 100);
        assert_eq!(cal.par_g2, -5000);
        assert_eq!(cal.par_g3, 18);
        assert_eq!(cal.res_heat_range, 1);
        assert_eq!(cal.res_heat_val, 40);
        assert_eq!(cal.range_sw_err, -1);
        let field = decode_field(&image.field).unwrap();
        assert!(field.new_data);
        assert!(!field.measuring);
        assert_eq!(field.temp_adc, 519_888);
        assert_eq!(field.press_adc, 375_000);
        assert_eq!(field.hum_adc, 25_000);
        assert_eq!(field.gas_adc, 400);
        assert_eq!(field.gas_range, 4);
        assert!(field.gas_valid && field.heat_stable);
    }

    #[test]
    fn synthetic_image_matches_the_datasheet_float_formulas() {
        // Independent evaluation of §3.3.1–3.3.3 and §3.4.1 on the bytes above.
        // Temperature 31.589253301313146 °C, pressure 95523.80328050393 Pa,
        // humidity 80.28193803615457 %RH, gas 545243.7995359771 Ohm.
        let r = image_reading();
        assert!(r.gaps.is_empty(), "{:?}", r.gaps);
        assert!((r.temperature_c.unwrap() - 31.589_253_301_313_146).abs() < 1e-9);
        assert!((r.pressure_pa.unwrap() - 95_523.803_280_503_93).abs() < 1e-6);
        assert!((r.humidity_rh.unwrap() - 80.281_938_036_154_57).abs() < 1e-9);
        assert!((r.gas_ohm.unwrap() - 545_243.799_535_977_1).abs() < 1e-4);
    }

    #[test]
    fn heater_code_for_the_synthetic_calibration_is_150() {
        // §3.3.5 at 300 °C with ambient 25 °C on the synthetic coefficients
        // evaluates to about 150.17, which becomes register code 150.
        let cal = decode_calib(
            &synthetic_image().coeff1,
            &synthetic_image().coeff2,
            &synthetic_image().coeff3,
        )
        .unwrap();
        assert_eq!(res_heat_code(&cal, 300, 25).unwrap(), 150);
    }

    #[test]
    fn gas_without_valid_and_stable_bits_is_absent_not_zero() {
        let image = synthetic_image();
        let cal = decode_calib(&image.coeff1, &image.coeff2, &image.coeff3).unwrap();
        let mut field = decode_field(&image.field).unwrap();
        field.gas_valid = false;
        let r = compensate(&cal, &field);
        assert!(r.gas_ohm.is_none());
        assert!(r.temperature_c.is_some());
        assert!(r.gaps.iter().any(|g| g.contains("gas_valid_r")));
        field.gas_valid = true;
        field.heat_stable = false;
        let r = compensate(&cal, &field);
        assert!(r.gas_ohm.is_none());
        assert!(r.gaps.iter().any(|g| g.contains("heat_stab_r")));
    }

    #[test]
    fn skipped_temperature_does_not_invent_pressure_or_humidity() {
        let image = synthetic_image();
        let cal = decode_calib(&image.coeff1, &image.coeff2, &image.coeff3).unwrap();
        let mut field = decode_field(&image.field).unwrap();
        field.temp_adc = TEMP_SKIPPED;
        let r = compensate(&cal, &field);
        assert!(r.temperature_c.is_none());
        assert!(r.pressure_pa.is_none());
        assert!(r.humidity_rh.is_none());
        assert!(r.gas_ohm.is_some(), "gas does not need t_fine");
    }

    #[test]
    fn short_blocks_are_refused() {
        assert!(decode_calib(&[], &[], &[]).is_err());
        assert!(decode_field(&[0x80]).is_err());
        let mut image = synthetic_image();
        image.field[0] = 0x00;
        let err = reading_from_image(&image).unwrap_err();
        assert!(err.contains("new_data_0"), "{err}");
    }

    #[test]
    fn chip_id_constant_is_the_datasheet_reset_value() {
        // Table 20, register Id at I2C 0xD0, reset state 0x61.
        assert_eq!(CHIP_ID, 0x61);
        assert_eq!(REG_CHIP_ID, 0xD0);
    }
}
