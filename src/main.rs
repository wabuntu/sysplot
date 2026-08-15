mod sadf;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph};
use sadf::HostReport;
use std::path::PathBuf;
use std::time::Duration;

/// View sysstat (.sar) files as terminal graphs, via sadf.
#[derive(Parser, Debug)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Args {
    /// Path to a sysstat .sar/saXX file (e.g. /var/log/sysstat/sa15)
    file: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum View {
    Cpu,
    Memory,
    Disk,
    Network,
}

impl View {
    const ALL: [View; 4] = [View::Cpu, View::Memory, View::Disk, View::Network];

    fn title(self) -> &'static str {
        match self {
            View::Cpu => "CPU",
            View::Memory => "Memory",
            View::Disk => "Disk",
            View::Network => "Network",
        }
    }

    fn next(self) -> View {
        let i = View::ALL.iter().position(|v| *v == self).unwrap();
        View::ALL[(i + 1) % View::ALL.len()]
    }

    fn prev(self) -> View {
        let i = View::ALL.iter().position(|v| *v == self).unwrap();
        View::ALL[(i + View::ALL.len() - 1) % View::ALL.len()]
    }
}

struct App {
    host: HostReport,
    view: View,
    disk_devices: Vec<String>,
    disk_idx: usize,
    net_ifaces: Vec<String>,
    net_idx: usize,
}

impl App {
    fn new(host: HostReport) -> App {
        let mut disk_devices: Vec<String> = host
            .statistics
            .iter()
            .flat_map(|s| s.disk.iter().map(|d| d.disk_device.clone()))
            .collect();
        disk_devices.sort();
        disk_devices.dedup();

        let mut net_ifaces: Vec<String> = host
            .statistics
            .iter()
            .filter_map(|s| s.network.as_ref())
            .flat_map(|n| n.net_dev.iter().map(|d| d.iface.clone()))
            .collect();
        net_ifaces.sort();
        net_ifaces.dedup();

        App {
            host,
            view: View::Cpu,
            disk_devices,
            disk_idx: 0,
            net_ifaces,
            net_idx: 0,
        }
    }

    fn cpu_series(&self) -> Vec<(f64, f64)> {
        self.host
            .statistics
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.cpu_load
                    .iter()
                    .find(|c| c.cpu == "all")
                    .map(|c| (i as f64, 100.0 - c.idle))
            })
            .collect()
    }

    fn memory_series(&self) -> Vec<(f64, f64)> {
        self.host
            .statistics
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.memory.as_ref().map(|m| (i as f64, m.memused_percent)))
            .collect()
    }

    fn disk_series(&self) -> Vec<(f64, f64)> {
        let Some(device) = self.disk_devices.get(self.disk_idx) else {
            return vec![];
        };
        self.host
            .statistics
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.disk
                    .iter()
                    .find(|d| &d.disk_device == device)
                    .map(|d| (i as f64, d.util_percent))
            })
            .collect()
    }

    fn network_series(&self) -> Vec<(f64, f64)> {
        let Some(iface) = self.net_ifaces.get(self.net_idx) else {
            return vec![];
        };
        self.host
            .statistics
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.network
                    .as_ref()
                    .and_then(|n| n.net_dev.iter().find(|d| &d.iface == iface))
                    .map(|d| (i as f64, d.rx_kb + d.tx_kb))
            })
            .collect()
    }

    fn date_label(&self) -> &str {
        self.host
            .statistics
            .first()
            .map(|s| s.timestamp.date.as_str())
            .unwrap_or("unknown date")
    }

    /// "HH:MM", or "MM-DD HH:MM" once the sample's date has moved past the
    /// first sample's date (a file can span a UTC day boundary).
    fn time_label(&self, sample_idx: usize) -> String {
        let Some(sample) = self.host.statistics.get(sample_idx) else {
            return String::new();
        };
        let time = sample
            .timestamp
            .time
            .rsplit_once(':')
            .map_or(sample.timestamp.time.as_str(), |(hm, _secs)| hm);
        if sample.timestamp.date == self.date_label() {
            time.to_string()
        } else {
            let month_day = sample
                .timestamp
                .date
                .split_once('-')
                .and_then(|(_year, rest)| rest.split_once('-'))
                .map_or_else(
                    || sample.timestamp.date.clone(),
                    |(m, d)| format!("{}-{}", m, d),
                );
            format!("{} {}", month_day, time)
        }
    }

    /// Selection label shown next to the view title (disk device / network
    /// interface), or an empty string for views with no sub-selection.
    fn selection_label(&self) -> String {
        match self.view {
            View::Disk => self
                .disk_devices
                .get(self.disk_idx)
                .cloned()
                .unwrap_or_else(|| "(no disks)".to_string()),
            View::Network => self
                .net_ifaces
                .get(self.net_idx)
                .cloned()
                .unwrap_or_else(|| "(no interfaces)".to_string()),
            View::Cpu | View::Memory => String::new(),
        }
    }

    fn cycle_selection(&mut self, forward: bool) {
        match self.view {
            View::Disk if !self.disk_devices.is_empty() => {
                let len = self.disk_devices.len();
                self.disk_idx = if forward {
                    (self.disk_idx + 1) % len
                } else {
                    (self.disk_idx + len - 1) % len
                };
            }
            View::Network if !self.net_ifaces.is_empty() => {
                let len = self.net_ifaces.len();
                self.net_idx = if forward {
                    (self.net_idx + 1) % len
                } else {
                    (self.net_idx + len - 1) % len
                };
            }
            _ => {}
        }
    }
}

fn main() {
    let args = Args::parse();

    let host = match sadf::load(&args.file) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if host.statistics.is_empty() {
        eprintln!("Error: {} contains no samples", args.file.display());
        std::process::exit(1);
    }

    let mut app = App::new(host);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Tab | KeyCode::Right => app.view = app.view.next(),
                KeyCode::BackTab | KeyCode::Left => app.view = app.view.prev(),
                KeyCode::Down => app.cycle_selection(true),
                KeyCode::Up => app.cycle_selection(false),
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(frame.area());

    let tabs: Vec<String> = View::ALL
        .iter()
        .map(|v| {
            if *v == app.view {
                format!("[{}]", v.title())
            } else {
                format!(" {} ", v.title())
            }
        })
        .collect();
    let header = format!(
        "{} — {} ({})   {}   q: quit, ←/→: switch, ↑/↓: select",
        app.host.nodename,
        app.date_label(),
        tabs.join(" "),
        app.selection_label()
    );
    frame.render_widget(Line::from(header), layout[0]);

    let (data, y_max, y_unit) = match app.view {
        View::Cpu => (app.cpu_series(), 100.0, "%"),
        View::Memory => (app.memory_series(), 100.0, "%"),
        View::Disk => (app.disk_series(), 100.0, "%"),
        View::Network => {
            let series = app.network_series();
            let max = series
                .iter()
                .map(|(_, y)| *y)
                .fold(0.0_f64, f64::max)
                .max(1.0);
            (series, max, "kB/s")
        }
    };

    if data.is_empty() {
        frame.render_widget(
            Paragraph::new("No data for this view").block(Block::bordered()),
            layout[1],
        );
        return;
    }

    let n = app.host.statistics.len().max(1);
    let x_bounds = [0.0, (n - 1) as f64];
    let mid = (n - 1) / 2;

    let dataset = Dataset::default()
        .name(app.view.title())
        .graph_type(GraphType::Line)
        .style(Style::default().cyan())
        .data(&data);

    let x_axis = Axis::default().bounds(x_bounds).labels([
        app.time_label(0),
        app.time_label(mid),
        app.time_label(n - 1),
    ]);

    let y_axis = Axis::default().title(y_unit).bounds([0.0, y_max]).labels([
        "0".to_string(),
        format!("{:.0}", y_max / 2.0),
        format!("{:.0}", y_max),
    ]);

    let chart = Chart::new(vec![dataset])
        .block(Block::bordered().title(app.view.title()))
        .x_axis(x_axis)
        .y_axis(y_axis);

    frame.render_widget(chart, layout[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sadf::{Disk, Sample, Timestamp};

    fn sample(date: &str, time: &str, disks: Vec<&str>) -> Sample {
        Sample {
            timestamp: Timestamp {
                date: date.to_string(),
                time: time.to_string(),
            },
            cpu_load: vec![],
            memory: None,
            disk: disks
                .into_iter()
                .map(|d| Disk {
                    disk_device: d.to_string(),
                    util_percent: 0.0,
                })
                .collect(),
            network: None,
        }
    }

    fn app_with_disks(disks: Vec<&str>) -> App {
        App::new(HostReport {
            nodename: "test-host".to_string(),
            statistics: vec![sample("2026-08-15", "10:00:00", disks)],
        })
    }

    #[test]
    fn view_cycles_forward_and_wraps() {
        assert_eq!(View::Cpu.next(), View::Memory);
        assert_eq!(View::Memory.next(), View::Disk);
        assert_eq!(View::Disk.next(), View::Network);
        assert_eq!(View::Network.next(), View::Cpu);
    }

    #[test]
    fn view_cycles_backward_and_wraps() {
        assert_eq!(View::Cpu.prev(), View::Network);
        assert_eq!(View::Network.prev(), View::Disk);
    }

    #[test]
    fn disk_selection_wraps_forward_and_backward() {
        let mut app = app_with_disks(vec!["vda", "vdb", "vdc"]);
        app.view = View::Disk;
        assert_eq!(app.selection_label(), "vda");

        app.cycle_selection(true);
        assert_eq!(app.selection_label(), "vdb");
        app.cycle_selection(true);
        assert_eq!(app.selection_label(), "vdc");
        app.cycle_selection(true);
        assert_eq!(
            app.selection_label(),
            "vda",
            "should wrap past the last device"
        );

        app.cycle_selection(false);
        assert_eq!(
            app.selection_label(),
            "vdc",
            "should wrap backward past the first device"
        );
    }

    #[test]
    fn selection_on_empty_disks_is_labeled_and_does_not_panic() {
        let mut app = app_with_disks(vec![]);
        app.view = View::Disk;
        assert_eq!(app.selection_label(), "(no disks)");
        app.cycle_selection(true);
        app.cycle_selection(false);
    }

    #[test]
    fn cycle_selection_on_cpu_view_is_a_no_op() {
        let mut app = app_with_disks(vec!["vda", "vdb"]);
        app.view = View::Cpu;
        app.cycle_selection(true);
        assert_eq!(app.disk_idx, 0);
    }

    #[test]
    fn time_label_shows_date_only_after_crossing_midnight() {
        let app = App::new(HostReport {
            nodename: "test-host".to_string(),
            statistics: vec![
                sample("2026-08-14", "23:50:00", vec![]),
                sample("2026-08-15", "00:10:00", vec![]),
            ],
        });
        assert_eq!(app.time_label(0), "23:50");
        assert_eq!(app.time_label(1), "08-15 00:10");
    }
}
