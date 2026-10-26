//! What the kernel says about the CAN interface: its controller state, bit rates and error
//! counters, from `ip -details -json link show <if>`, and its packet counters from sysfs.
//!
//! The cog never configures an interface. Bringing one up (`ip link set canX up type can bitrate
//! N`) is an operator action; this reads what is there and says so.

use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    /// `UP` / `DOWN` (operstate) as the kernel reports it.
    pub oper: String,
    /// Controller state: `ERROR-ACTIVE`, `ERROR-WARNING`, `ERROR-PASSIVE`, `BUS-OFF`, `STOPPED`,
    /// `SLEEPING`. Empty for a virtual interface.
    pub state: String,
    pub bitrate: Option<u64>,
    pub data_bitrate: Option<u64>,
    pub fd: bool,
    pub listen_only: bool,
    pub loopback: bool,
    pub mtu: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub restart_ms: u64,
    pub vcan: bool,
}

/// Parse `ip -details -json link show <if>`.
pub fn parse_ip_json(text: &str) -> Result<Info, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("ip output: {e}"))?;
    let l = v.get(0).ok_or("ip printed no interface")?;
    let kind = l["linkinfo"]["info_kind"].as_str().unwrap_or_default();
    let d = &l["linkinfo"]["info_data"];
    let ctrl: Vec<&str> = d["ctrlmode"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Ok(Info {
        oper: l["operstate"].as_str().unwrap_or("UNKNOWN").to_string(),
        state: d["state"].as_str().unwrap_or_default().to_string(),
        bitrate: d["bittiming"]["bitrate"].as_u64(),
        data_bitrate: d["data_bittiming"]["bitrate"].as_u64(),
        fd: ctrl.contains(&"FD"),
        listen_only: ctrl.contains(&"LISTEN-ONLY"),
        loopback: ctrl.contains(&"LOOPBACK"),
        mtu: l["mtu"].as_u64().unwrap_or(0),
        rx_errors: d["berr_counter"]["rx"].as_u64().unwrap_or(0),
        tx_errors: d["berr_counter"]["tx"].as_u64().unwrap_or(0),
        restart_ms: d["restart_ms"].as_u64().unwrap_or(0),
        vcan: kind == "vcan",
    })
}

pub fn read(name: &str) -> Result<Info, String> {
    let out = Command::new("ip")
        .args(["-details", "-json", "link", "show", name])
        .output()
        .map_err(|e| format!("ip: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ip link show {name}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_ip_json(&String::from_utf8_lossy(&out.stdout))
}

/// A packet counter from `/sys/class/net/<if>/statistics/<name>`.
pub fn stat(name: &str, counter: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/sys/class/net/{name}/statistics/{counter}"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Why this interface cannot be listened to, if it cannot: down, or not a CAN interface.
pub fn unusable(i: &Info) -> Option<String> {
    if i.oper == "DOWN" || i.state == "STOPPED" {
        return Some("the interface is down; bringing it up (ip link set ... up type can bitrate ...) is an operator action, not this cog's".into());
    }
    if i.state == "BUS-OFF" {
        return Some("the controller is bus-off".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAN0_UP: &str = r#"[{"ifindex":5,"ifname":"can0","flags":["NOARP","UP","LOWER_UP","ECHO"],"mtu":72,"operstate":"UP","link_type":"can",
      "linkinfo":{"info_kind":"can","info_data":{"ctrlmode":["FD","LISTEN-ONLY"],"state":"ERROR-ACTIVE","berr_counter":{"tx":3,"rx":1},
      "restart_ms":100,"bittiming":{"bitrate":500000,"sample_point":"0.875"},"data_bittiming":{"bitrate":2000000}}}}]"#;
    const CAN0_DOWN: &str = r#"[{"ifname":"can0","mtu":16,"operstate":"DOWN","link_type":"can","linkinfo":{"info_kind":"can","info_data":{"state":"STOPPED","berr_counter":{"tx":0,"rx":0},"restart_ms":0}}}]"#;
    const VCAN: &str = r#"[{"ifname":"vcan0","mtu":72,"operstate":"UNKNOWN","link_type":"can","linkinfo":{"info_kind":"vcan"}}]"#;

    #[test]
    fn an_up_fd_controller_reports_rates_state_and_counters() {
        let i = parse_ip_json(CAN0_UP).unwrap();
        assert_eq!(
            (i.oper.as_str(), i.state.as_str(), i.bitrate, i.data_bitrate),
            ("UP", "ERROR-ACTIVE", Some(500_000), Some(2_000_000))
        );
        assert!(i.fd && i.listen_only && !i.loopback && !i.vcan);
        assert_eq!(
            (i.mtu, i.rx_errors, i.tx_errors, i.restart_ms),
            (72, 1, 3, 100)
        );
        assert_eq!(unusable(&i), None);
    }

    #[test]
    fn a_down_interface_is_unusable_with_a_reason_that_names_the_operator() {
        let i = parse_ip_json(CAN0_DOWN).unwrap();
        assert_eq!((i.bitrate, i.mtu), (None, 16));
        assert!(unusable(&i).unwrap().contains("operator"));
        let off = Info {
            oper: "UP".into(),
            state: "BUS-OFF".into(),
            ..Info::default()
        };
        assert!(unusable(&off).unwrap().contains("bus-off"));
    }

    #[test]
    fn a_virtual_interface_has_no_controller() {
        let i = parse_ip_json(VCAN).unwrap();
        assert!(i.vcan && i.state.is_empty() && i.bitrate.is_none());
        assert_eq!(unusable(&i), None, "operstate UNKNOWN is fine for vcan");
    }

    #[test]
    fn junk_is_an_error() {
        assert!(parse_ip_json("not json").is_err());
        assert!(parse_ip_json("[]").is_err());
    }
}
