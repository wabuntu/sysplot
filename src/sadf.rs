use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct SadfReport {
    sysstat: SysstatRoot,
}

#[derive(Debug, Deserialize)]
struct SysstatRoot {
    hosts: Vec<HostReport>,
}

#[derive(Debug, Deserialize)]
pub struct HostReport {
    pub nodename: String,
    pub statistics: Vec<Sample>,
}

#[derive(Debug, Deserialize)]
pub struct Sample {
    pub timestamp: Timestamp,
    #[serde(rename = "cpu-load", default)]
    pub cpu_load: Vec<CpuLoad>,
    #[serde(default)]
    pub memory: Option<Memory>,
    #[serde(default)]
    pub disk: Vec<Disk>,
    #[serde(default)]
    pub network: Option<Network>,
}

#[derive(Debug, Deserialize)]
pub struct Timestamp {
    pub date: String,
    pub time: String,
}

#[derive(Debug, Deserialize)]
pub struct CpuLoad {
    pub cpu: String,
    pub idle: f64,
}

#[derive(Debug, Deserialize)]
pub struct Memory {
    #[serde(rename = "memused-percent")]
    pub memused_percent: f64,
}

#[derive(Debug, Deserialize)]
pub struct Disk {
    #[serde(rename = "disk-device")]
    pub disk_device: String,
    #[serde(rename = "util-percent")]
    pub util_percent: f64,
}

#[derive(Debug, Deserialize)]
pub struct Network {
    #[serde(rename = "net-dev", default)]
    pub net_dev: Vec<NetDev>,
}

#[derive(Debug, Deserialize)]
pub struct NetDev {
    pub iface: String,
    #[serde(rename = "rxkB")]
    pub rx_kb: f64,
    #[serde(rename = "txkB")]
    pub tx_kb: f64,
}

/// Run `sadf -j -- -A <file>` and parse the first host's statistics.
/// sadf itself does the sar-binary-format parsing, so this only has to
/// handle sysstat's documented, versioned JSON schema.
pub fn load(path: &Path) -> Result<HostReport, String> {
    let output = Command::new("sadf")
        .arg("-j")
        .arg("--")
        .arg("-A")
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run sadf (is sysstat installed?): {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "sadf exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let report: SadfReport = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("failed to parse sadf JSON output: {}", e))?;

    report
        .sysstat
        .hosts
        .into_iter()
        .next()
        .ok_or_else(|| "sadf reported no hosts in this file".to_string())
}
