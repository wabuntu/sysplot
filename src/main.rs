mod sadf;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, List, ListState, Paragraph};
use sadf::Sample;
use std::path::PathBuf;
use std::time::Duration;

const MAX_DAYS: usize = 30;
const DEFAULT_SYSSTAT_DIR: &str = "/var/log/sysstat";

/// View sysstat (.sar) history as terminal graphs, via sadf.
#[derive(Parser, Debug)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Args {
    /// A sysstat saXX/saYYYYMMDD file, or a directory of them
    /// (defaults to /var/log/sysstat)
    path: Option<PathBuf>,
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

/// What slice of the timeline a Metric screen is showing: a whole calendar
/// day, or one hour drilled into from that day.
#[derive(Clone, Debug)]
enum Scope {
    Day(String),
    Hour(String, String),
}

impl Scope {
    fn date(&self) -> &str {
        match self {
            Scope::Day(d) | Scope::Hour(d, _) => d,
        }
    }
}

struct MetricState {
    scope: Scope,
    view: View,
    disk_devices: Vec<String>,
    disk_idx: usize,
    net_ifaces: Vec<String>,
    net_idx: usize,
}

enum Screen {
    DayList {
        state: ListState,
    },
    Metric(MetricState),
    HourList {
        date: String,
        hours: Vec<String>,
        state: ListState,
    },
}

struct App {
    nodename: String,
    samples: Vec<Sample>,
    /// Distinct calendar dates present in `samples`, most recent first.
    days: Vec<String>,
    screen: Screen,
}

impl App {
    fn new(nodename: String, samples: Vec<Sample>) -> App {
        let mut days: Vec<String> = samples.iter().map(|s| s.timestamp.date.clone()).collect();
        days.sort();
        days.dedup();
        days.reverse();

        let mut state = ListState::default();
        if !days.is_empty() {
            state.select(Some(0));
        }

        App {
            nodename,
            samples,
            days,
            screen: Screen::DayList { state },
        }
    }

    fn enter_day(&mut self, date: String) {
        let filtered = filter_samples(&self.samples, &Scope::Day(date.clone()));
        let (disk_devices, net_ifaces) = device_lists(&filtered);
        self.screen = Screen::Metric(MetricState {
            scope: Scope::Day(date),
            view: View::Cpu,
            disk_devices,
            disk_idx: 0,
            net_ifaces,
            net_idx: 0,
        });
    }

    fn enter_hour(&mut self, date: String, hour: String) {
        let scope = Scope::Hour(date, hour);
        let filtered = filter_samples(&self.samples, &scope);
        let (disk_devices, net_ifaces) = device_lists(&filtered);
        self.screen = Screen::Metric(MetricState {
            scope,
            view: View::Cpu,
            disk_devices,
            disk_idx: 0,
            net_ifaces,
            net_idx: 0,
        });
    }

    fn open_hour_list(&mut self, date: String) {
        let hours = hours_for_date(&self.samples, &date);
        let mut state = ListState::default();
        if !hours.is_empty() {
            state.select(Some(0));
        }
        self.screen = Screen::HourList { date, hours, state };
    }

    fn back_to_hour_list(&mut self, date: String, selected_hour: &str) {
        let hours = hours_for_date(&self.samples, &date);
        let idx = hours.iter().position(|h| h == selected_hour).unwrap_or(0);
        let mut state = ListState::default();
        if !hours.is_empty() {
            state.select(Some(idx));
        }
        self.screen = Screen::HourList { date, hours, state };
    }

    fn back_to_day_list(&mut self, date: &str) {
        let idx = self.days.iter().position(|d| d == date).unwrap_or(0);
        let mut state = ListState::default();
        if !self.days.is_empty() {
            state.select(Some(idx));
        }
        self.screen = Screen::DayList { state };
    }
}

fn filter_samples<'a>(samples: &'a [Sample], scope: &Scope) -> Vec<&'a Sample> {
    match scope {
        Scope::Day(date) => samples
            .iter()
            .filter(|s| &s.timestamp.date == date)
            .collect(),
        Scope::Hour(date, hour) => samples
            .iter()
            .filter(|s| &s.timestamp.date == date && s.timestamp.time.starts_with(hour.as_str()))
            .collect(),
    }
}

fn hours_for_date(samples: &[Sample], date: &str) -> Vec<String> {
    let mut hours: Vec<String> = samples
        .iter()
        .filter(|s| s.timestamp.date == date)
        .map(|s| s.timestamp.time[0..2].to_string())
        .collect();
    hours.sort();
    hours.dedup();
    hours
}

fn device_lists(samples: &[&Sample]) -> (Vec<String>, Vec<String>) {
    let mut disk_devices: Vec<String> = samples
        .iter()
        .flat_map(|s| s.disk.iter().map(|d| d.disk_device.clone()))
        .collect();
    disk_devices.sort();
    disk_devices.dedup();

    let mut net_ifaces: Vec<String> = samples
        .iter()
        .filter_map(|s| s.network.as_ref())
        .flat_map(|n| n.net_dev.iter().map(|d| d.iface.clone()))
        .collect();
    net_ifaces.sort();
    net_ifaces.dedup();

    (disk_devices, net_ifaces)
}

fn cycle_metric_selection(m: &mut MetricState, forward: bool) {
    match m.view {
        View::Disk if !m.disk_devices.is_empty() => {
            let len = m.disk_devices.len();
            m.disk_idx = if forward {
                (m.disk_idx + 1) % len
            } else {
                (m.disk_idx + len - 1) % len
            };
        }
        View::Network if !m.net_ifaces.is_empty() => {
            let len = m.net_ifaces.len();
            m.net_idx = if forward {
                (m.net_idx + 1) % len
            } else {
                (m.net_idx + len - 1) % len
            };
        }
        _ => {}
    }
}

fn move_selection(state: &mut ListState, len: usize, forward: bool) {
    if len == 0 {
        return;
    }
    let i = state.selected().unwrap_or(0);
    let next = if forward {
        (i + 1) % len
    } else {
        (i + len - 1) % len
    };
    state.select(Some(next));
}

fn cpu_series(samples: &[&Sample]) -> Vec<(f64, f64)> {
    samples
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

fn memory_series(samples: &[&Sample]) -> Vec<(f64, f64)> {
    samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.memory.as_ref().map(|m| (i as f64, m.memused_percent)))
        .collect()
}

fn disk_series(samples: &[&Sample], device: &str) -> Vec<(f64, f64)> {
    samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            s.disk
                .iter()
                .find(|d| d.disk_device == device)
                .map(|d| (i as f64, d.util_percent))
        })
        .collect()
}

fn network_series(samples: &[&Sample], iface: &str) -> Vec<(f64, f64)> {
    samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            s.network
                .as_ref()
                .and_then(|n| n.net_dev.iter().find(|d| d.iface == iface))
                .map(|d| (i as f64, d.rx_kb + d.tx_kb))
        })
        .collect()
}

/// "HH:MM", or "MM-DD HH:MM" once the sample's date has moved past
/// `base_date` (a day scope can span a UTC day boundary).
fn time_label(samples: &[&Sample], idx: usize, base_date: &str) -> String {
    let Some(sample) = samples.get(idx) else {
        return String::new();
    };
    let time = sample
        .timestamp
        .time
        .rsplit_once(':')
        .map_or(sample.timestamp.time.as_str(), |(hm, _secs)| hm);
    if sample.timestamp.date == base_date {
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

fn main() {
    let args = Args::parse();
    let path = args
        .path
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SYSSTAT_DIR));

    let (nodename, samples) = match sadf::load(&path, MAX_DAYS) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    if samples.is_empty() {
        eprintln!("Error: no samples found in {}", path.display());
        std::process::exit(1);
    }

    let mut app = App::new(nodename, samples);

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
            if key.code == KeyCode::Char('q') {
                return Ok(());
            }
            handle_key(app, key.code);
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode) {
    match &mut app.screen {
        Screen::DayList { state } => match code {
            KeyCode::Down => move_selection(state, app.days.len(), true),
            KeyCode::Up => move_selection(state, app.days.len(), false),
            KeyCode::Enter => {
                if let Some(date) = state.selected().and_then(|i| app.days.get(i)).cloned() {
                    app.enter_day(date);
                }
            }
            _ => {}
        },
        Screen::Metric(m) => match code {
            KeyCode::Right | KeyCode::Tab => m.view = m.view.next(),
            KeyCode::Left | KeyCode::BackTab => m.view = m.view.prev(),
            KeyCode::Down => cycle_metric_selection(m, true),
            KeyCode::Up => cycle_metric_selection(m, false),
            KeyCode::Enter => {
                if let Scope::Day(date) = &m.scope {
                    let date = date.clone();
                    app.open_hour_list(date);
                }
            }
            KeyCode::Esc => match m.scope.clone() {
                Scope::Day(date) => app.back_to_day_list(&date),
                Scope::Hour(date, hour) => app.back_to_hour_list(date, &hour),
            },
            _ => {}
        },
        Screen::HourList { date, hours, state } => match code {
            KeyCode::Down => move_selection(state, hours.len(), true),
            KeyCode::Up => move_selection(state, hours.len(), false),
            KeyCode::Enter => {
                if let Some(hour) = state.selected().and_then(|i| hours.get(i)).cloned() {
                    let date = date.clone();
                    app.enter_hour(date, hour);
                }
            }
            KeyCode::Esc => {
                let date = date.clone();
                app.enter_day(date);
            }
            _ => {}
        },
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let nodename = app.nodename.clone();
    let days = app.days.clone();
    let samples = &app.samples;
    match &mut app.screen {
        Screen::DayList { state } => draw_day_list(frame, &nodename, &days, samples, state),
        Screen::HourList { date, hours, state } => draw_hour_list(frame, date, hours, state),
        Screen::Metric(m) => {
            let filtered = filter_samples(samples, &m.scope);
            draw_metric(frame, &nodename, m, &filtered);
        }
    }
}

fn draw_day_list(
    frame: &mut Frame,
    nodename: &str,
    days: &[String],
    samples: &[Sample],
    state: &mut ListState,
) {
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(frame.area());
    let header = format!(
        "{} — {} day(s) of sysstat data   ↑/↓: select, Enter: open, q: quit",
        nodename,
        days.len()
    );
    frame.render_widget(Line::from(header), layout[0]);

    if days.is_empty() {
        frame.render_widget(
            Paragraph::new("No sysstat data files found").block(Block::bordered()),
            layout[1],
        );
        return;
    }

    let items: Vec<String> = days
        .iter()
        .map(|d| {
            let n = samples.iter().filter(|s| &s.timestamp.date == d).count();
            format!("{}   ({} samples)", d, n)
        })
        .collect();

    let list = List::new(items)
        .block(Block::bordered().title("Days"))
        .highlight_style(Style::default().cyan().bold())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, layout[1], state);
}

fn draw_hour_list(frame: &mut Frame, date: &str, hours: &[String], state: &mut ListState) {
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(frame.area());
    let header = format!(
        "{} — hours   ↑/↓: select, Enter: open, Esc: back, q: quit",
        date
    );
    frame.render_widget(Line::from(header), layout[0]);

    if hours.is_empty() {
        frame.render_widget(
            Paragraph::new("No samples for this day").block(Block::bordered()),
            layout[1],
        );
        return;
    }

    let items: Vec<String> = hours.iter().map(|h| format!("{}:00", h)).collect();
    let list = List::new(items)
        .block(Block::bordered().title("Hours"))
        .highlight_style(Style::default().cyan().bold())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, layout[1], state);
}

fn draw_metric(frame: &mut Frame, nodename: &str, m: &MetricState, samples: &[&Sample]) {
    let layout = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(frame.area());

    let tabs: Vec<String> = View::ALL
        .iter()
        .map(|v| {
            if *v == m.view {
                format!("[{}]", v.title())
            } else {
                format!(" {} ", v.title())
            }
        })
        .collect();

    let scope_label = match &m.scope {
        Scope::Day(date) => date.clone(),
        Scope::Hour(date, hour) => format!("{} {}:00", date, hour),
    };
    let selection = match m.view {
        View::Disk => m
            .disk_devices
            .get(m.disk_idx)
            .cloned()
            .unwrap_or_else(|| "(no disks)".to_string()),
        View::Network => m
            .net_ifaces
            .get(m.net_idx)
            .cloned()
            .unwrap_or_else(|| "(no interfaces)".to_string()),
        View::Cpu | View::Memory => String::new(),
    };
    let drill_hint = match m.scope {
        Scope::Day(_) => "Enter: hour view, ",
        Scope::Hour(..) => "",
    };

    let header = format!(
        "{} — {} ({})   {}   {}Esc: back, ←/→: switch, ↑/↓: select, q: quit",
        nodename,
        scope_label,
        tabs.join(" "),
        selection,
        drill_hint
    );
    frame.render_widget(Line::from(header), layout[0]);

    let disk_name = m.disk_devices.get(m.disk_idx).map_or("", String::as_str);
    let net_name = m.net_ifaces.get(m.net_idx).map_or("", String::as_str);
    let (data, y_max, y_unit) = match m.view {
        View::Cpu => (cpu_series(samples), 100.0, "%"),
        View::Memory => (memory_series(samples), 100.0, "%"),
        View::Disk => (disk_series(samples, disk_name), 100.0, "%"),
        View::Network => {
            let series = network_series(samples, net_name);
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

    let n = samples.len().max(1);
    let x_bounds = [0.0, (n - 1) as f64];
    let mid = (n - 1) / 2;
    let base_date = m.scope.date();

    let dataset = Dataset::default()
        .name(m.view.title())
        .graph_type(GraphType::Line)
        .style(Style::default().cyan())
        .data(&data);

    let x_axis = Axis::default().bounds(x_bounds).labels([
        time_label(samples, 0, base_date),
        time_label(samples, mid, base_date),
        time_label(samples, n - 1, base_date),
    ]);

    let y_axis = Axis::default().title(y_unit).bounds([0.0, y_max]).labels([
        "0".to_string(),
        format!("{:.0}", y_max / 2.0),
        format!("{:.0}", y_max),
    ]);

    let chart = Chart::new(vec![dataset])
        .block(Block::bordered().title(m.view.title()))
        .x_axis(x_axis)
        .y_axis(y_axis);
    frame.render_widget(chart, layout[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sadf::{Disk, Timestamp};

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
    fn days_are_deduped_and_sorted_most_recent_first() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-03", "10:00:00", vec![]),
            sample("2026-08-01", "11:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let app = App::new("host".to_string(), samples);
        assert_eq!(app.days, vec!["2026-08-03", "2026-08-02", "2026-08-01"]);
    }

    #[test]
    fn filter_samples_scopes_by_day_and_hour() {
        let samples = vec![
            sample("2026-08-01", "09:10:00", vec![]),
            sample("2026-08-01", "10:10:00", vec![]),
            sample("2026-08-01", "10:40:00", vec![]),
            sample("2026-08-02", "10:10:00", vec![]),
        ];
        let day = filter_samples(&samples, &Scope::Day("2026-08-01".to_string()));
        assert_eq!(day.len(), 3);

        let hour = filter_samples(
            &samples,
            &Scope::Hour("2026-08-01".to_string(), "10".to_string()),
        );
        assert_eq!(hour.len(), 2);
    }

    #[test]
    fn hours_for_date_are_deduped_and_sorted() {
        let samples = vec![
            sample("2026-08-01", "10:10:00", vec![]),
            sample("2026-08-01", "10:40:00", vec![]),
            sample("2026-08-01", "09:00:00", vec![]),
            sample("2026-08-02", "23:00:00", vec![]),
        ];
        assert_eq!(hours_for_date(&samples, "2026-08-01"), vec!["09", "10"]);
    }

    #[test]
    fn disk_selection_wraps_forward_and_backward() {
        let mut m = MetricState {
            scope: Scope::Day("2026-08-01".to_string()),
            view: View::Disk,
            disk_devices: vec!["vda".to_string(), "vdb".to_string(), "vdc".to_string()],
            disk_idx: 0,
            net_ifaces: vec![],
            net_idx: 0,
        };
        cycle_metric_selection(&mut m, true);
        assert_eq!(m.disk_idx, 1);
        cycle_metric_selection(&mut m, true);
        assert_eq!(m.disk_idx, 2);
        cycle_metric_selection(&mut m, true);
        assert_eq!(m.disk_idx, 0, "should wrap past the last device");
        cycle_metric_selection(&mut m, false);
        assert_eq!(m.disk_idx, 2, "should wrap backward past the first device");
    }

    #[test]
    fn cycle_selection_on_empty_devices_does_not_panic() {
        let mut m = MetricState {
            scope: Scope::Day("2026-08-01".to_string()),
            view: View::Disk,
            disk_devices: vec![],
            disk_idx: 0,
            net_ifaces: vec![],
            net_idx: 0,
        };
        cycle_metric_selection(&mut m, true);
        cycle_metric_selection(&mut m, false);
        assert_eq!(m.disk_idx, 0);
    }

    #[test]
    fn cycle_selection_on_cpu_view_is_a_no_op() {
        let mut m = MetricState {
            scope: Scope::Day("2026-08-01".to_string()),
            view: View::Cpu,
            disk_devices: vec!["vda".to_string(), "vdb".to_string()],
            disk_idx: 0,
            net_ifaces: vec![],
            net_idx: 0,
        };
        cycle_metric_selection(&mut m, true);
        assert_eq!(m.disk_idx, 0);
    }

    #[test]
    fn time_label_shows_date_only_after_crossing_midnight() {
        let samples = [
            sample("2026-08-14", "23:50:00", vec![]),
            sample("2026-08-15", "00:10:00", vec![]),
        ];
        let refs: Vec<&Sample> = samples.iter().collect();
        assert_eq!(time_label(&refs, 0, "2026-08-14"), "23:50");
        assert_eq!(time_label(&refs, 1, "2026-08-14"), "08-15 00:10");
    }
}
