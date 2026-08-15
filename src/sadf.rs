use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

#[derive(Debug, Deserialize)]
struct SadfReport {
    sysstat: SysstatRoot,
}

#[derive(Debug, Deserialize)]
struct SysstatRoot {
    hosts: Vec<HostReport>,
}

#[derive(Debug, Deserialize)]
struct HostReport {
    nodename: String,
    statistics: Vec<Sample>,
}

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
pub struct Timestamp {
    pub date: String,
    pub time: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CpuLoad {
    pub cpu: String,
    pub idle: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Memory {
    #[serde(rename = "memused-percent")]
    pub memused_percent: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Disk {
    #[serde(rename = "disk-device")]
    pub disk_device: String,
    #[serde(rename = "util-percent")]
    pub util_percent: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Network {
    #[serde(rename = "net-dev", default)]
    pub net_dev: Vec<NetDev>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NetDev {
    pub iface: String,
    #[serde(rename = "rxkB")]
    pub rx_kb: f64,
    #[serde(rename = "txkB")]
    pub tx_kb: f64,
}

/// A sysstat data file's name is either `saDD` (day of the current month) or
/// `saYYYYMMDD` (rotated out of the current month). Anything else in the
/// directory (saved reports, lock files, ...) is not one of these.
fn looks_like_sar_data_file(name: &str) -> bool {
    let Some(digits) = name.strip_prefix("sa") else {
        return false;
    };
    (digits.len() == 2 || digits.len() == 8) && digits.bytes().all(|b| b.is_ascii_digit())
}

pub fn discover_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {}", dir.display(), e))?;

    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(looks_like_sar_data_file)
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    Ok(files)
}

/// Run `sadf -j -- -A <file>` and parse the first host's statistics.
/// sadf itself does the sar-binary-format parsing, so this only has to
/// handle sysstat's documented, versioned JSON schema.
fn load_file(path: &Path) -> Result<(String, Vec<Sample>), String> {
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

    let host = report
        .sysstat
        .hosts
        .into_iter()
        .next()
        .ok_or_else(|| "sadf reported no hosts in this file".to_string())?;
    Ok((host.nodename, host.statistics))
}

/// Load a single sar file, or every sar file in a directory (run through
/// `sadf` in parallel, since a full history can be dozens of files), merged
/// into one timeline and capped to the most recent `max_days` calendar days.
pub fn load(path: &Path, max_days: usize) -> Result<(String, Vec<Sample>), String> {
    let files = if path.is_dir() {
        discover_files(path)?
    } else {
        vec![path.to_path_buf()]
    };
    if files.is_empty() {
        return Err(format!("no sysstat data files found in {}", path.display()));
    }

    let results: Vec<Result<(String, Vec<Sample>), String>> = thread::scope(|scope| {
        let handles: Vec<_> = files
            .iter()
            .map(|f| scope.spawn(move || load_file(f)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut nodename = None;
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok((name, mut s)) => {
                nodename.get_or_insert(name);
                samples.append(&mut s);
            }
            Err(e) => errors.push(e),
        }
    }

    let Some(nodename) = nodename else {
        return Err(format!(
            "failed to read any sysstat data file: {}",
            errors.join("; ")
        ));
    };
    if !errors.is_empty() {
        eprintln!(
            "Warning: {} of {} files failed to load: {}",
            errors.len(),
            files.len(),
            errors.join("; ")
        );
    }

    samples.sort_by(|a, b| {
        (&a.timestamp.date, &a.timestamp.time).cmp(&(&b.timestamp.date, &b.timestamp.time))
    });

    let mut dates: Vec<&str> = samples.iter().map(|s| s.timestamp.date.as_str()).collect();
    dates.sort_unstable();
    dates.dedup();
    if dates.len() > max_days {
        let cutoff = dates[dates.len() - max_days].to_string();
        samples.retain(|s| s.timestamp.date >= cutoff);
    }

    Ok((nodename, samples))
}
