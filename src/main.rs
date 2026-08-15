mod sadf;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::symbols;
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

/// What slice of the timeline a Metric screen is showing: every loaded
/// sample, a single calendar day, or one hour drilled into from a day.
#[derive(Clone, Debug)]
enum Scope {
    All,
    Day(String),
    Hour(String, String),
}

struct MetricState {
    scope: Scope,
    view: View,
    disk_devices: Vec<String>,
    disk_idx: usize,
    net_ifaces: Vec<String>,
    net_idx: usize,
    /// Index into `App::days` of the day highlighted on the Overview chart.
    /// Meaningless outside `Scope::All`.
    day_cursor: usize,
}

enum Screen {
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
    /// Distinct calendar dates present in `samples`, oldest first (matches
    /// the Overview chart's left-to-right x-axis).
    days: Vec<String>,
    screen: Screen,
}

impl App {
    fn new(nodename: String, samples: Vec<Sample>) -> App {
        let mut days: Vec<String> = samples.iter().map(|s| s.timestamp.date.clone()).collect();
        days.sort();
        days.dedup();

        let cursor = days.len().saturating_sub(1);
        let screen = Screen::Metric(overview_metric_state(&samples, cursor));

        App {
            nodename,
            samples,
            days,
            screen,
        }
    }

    /// Back to the Overview, cursor parked on `date` (used when Esc-ing out
    /// of a day view, so you land back where you drilled in from).
    fn back_to_overview_at(&mut self, date: &str) {
        let idx = self
            .days
            .iter()
            .position(|d| d == date)
            .unwrap_or(self.days.len().saturating_sub(1));
        self.screen = Screen::Metric(overview_metric_state(&self.samples, idx));
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
            day_cursor: 0,
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
            day_cursor: 0,
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
}

fn filter_samples<'a>(samples: &'a [Sample], scope: &Scope) -> Vec<&'a Sample> {
    match scope {
        Scope::All => samples.iter().collect(),
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

fn overview_metric_state(samples: &[Sample], day_cursor: usize) -> MetricState {
    let filtered: Vec<&Sample> = samples.iter().collect();
    let (disk_devices, net_ifaces) = device_lists(&filtered);
    MetricState {
        scope: Scope::All,
        view: View::Cpu,
        disk_devices,
        disk_idx: 0,
        net_ifaces,
        net_idx: 0,
        day_cursor,
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

/// The value a sample contributes to the currently selected view, or None if
/// that sample has nothing for it (e.g. a disk device that only appears on
/// some days).
fn metric_value(s: &Sample, view: View, disk_name: &str, net_name: &str) -> Option<f64> {
    match view {
        View::Cpu => s
            .cpu_load
            .iter()
            .find(|c| c.cpu == "all")
            .map(|c| 100.0 - c.idle),
        View::Memory => s.memory.as_ref().map(|m| m.memused_percent),
        View::Disk => s
            .disk
            .iter()
            .find(|d| d.disk_device == disk_name)
            .map(|d| d.util_percent),
        View::Network => s
            .network
            .as_ref()
            .and_then(|n| n.net_dev.iter().find(|d| d.iface == net_name))
            .map(|d| d.rx_kb + d.tx_kb),
    }
}

/// Raw per-sample series, one point per sample in order (used by the Day and
/// Hour views, where every sample is already the finest resolution there is).
fn series(samples: &[&Sample], view: View, disk_name: &str, net_name: &str) -> Vec<(f64, f64)> {
    samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| metric_value(s, view, disk_name, net_name).map(|v| (i as f64, v)))
        .collect()
}

type MaxAvgMinSeries = (Vec<(f64, f64)>, Vec<(f64, f64)>, Vec<(f64, f64)>);

/// Per-day max/average/min series, one point per day in `days_chronological`
/// (oldest first, for a left-to-right timeline). Used by the Overview, which
/// would otherwise have to cram thousands of raw samples onto one chart.
fn daily_series(
    samples: &[Sample],
    days_chronological: &[String],
    view: View,
    disk_name: &str,
    net_name: &str,
) -> MaxAvgMinSeries {
    let mut max_series = Vec::new();
    let mut avg_series = Vec::new();
    let mut min_series = Vec::new();
    for (i, day) in days_chronological.iter().enumerate() {
        let values: Vec<f64> = samples
            .iter()
            .filter(|s| &s.timestamp.date == day)
            .filter_map(|s| metric_value(s, view, disk_name, net_name))
            .collect();
        if values.is_empty() {
            continue;
        }
        let max = values.iter().copied().fold(f64::MIN, f64::max);
        let min = values.iter().copied().fold(f64::MAX, f64::min);
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        max_series.push((i as f64, max));
        avg_series.push((i as f64, avg));
        min_series.push((i as f64, min));
    }
    (max_series, avg_series, min_series)
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

/// Date label ("MM-DD") for the Overview's per-day x-axis.
fn date_label(days_chronological: &[String], idx: usize) -> String {
    let Some(date) = days_chronological.get(idx) else {
        return String::new();
    };
    date.split_once('-')
        .and_then(|(_year, rest)| rest.split_once('-'))
        .map_or_else(|| date.clone(), |(m, d)| format!("{}-{}", m, d))
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
        Screen::Metric(m) if matches!(m.scope, Scope::All) => match code {
            KeyCode::Tab => m.view = m.view.next(),
            KeyCode::BackTab => m.view = m.view.prev(),
            KeyCode::Right => {
                let max = app.days.len().saturating_sub(1);
                m.day_cursor = (m.day_cursor + 1).min(max);
            }
            KeyCode::Left => m.day_cursor = m.day_cursor.saturating_sub(1),
            KeyCode::Down => cycle_metric_selection(m, true),
            KeyCode::Up => cycle_metric_selection(m, false),
            KeyCode::Enter => {
                if let Some(date) = app.days.get(m.day_cursor).cloned() {
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
                Scope::All => unreachable!("handled by the Scope::All arm above"),
                Scope::Day(date) => app.back_to_overview_at(&date),
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
        Screen::HourList { date, hours, state } => draw_hour_list(frame, date, hours, state),
        Screen::Metric(m) => {
            let filtered = filter_samples(samples, &m.scope);
            draw_metric(frame, &nodename, m, samples, &filtered, &days);
        }
    }
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

fn draw_metric(
    frame: &mut Frame,
    nodename: &str,
    m: &MetricState,
    all_samples: &[Sample],
    filtered: &[&Sample],
    days: &[String],
) {
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
        Scope::All => {
            let selected = days.get(m.day_cursor).map_or("", String::as_str);
            format!("{} of {} days — {}", m.day_cursor + 1, days.len(), selected)
        }
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
    let nav_hint = match m.scope {
        Scope::All => "←/→: day, Tab: view, ↑/↓: select, Enter: open day, ",
        Scope::Day(_) => "←/→: view, ↑/↓: select, Enter: hour view, Esc: back, ",
        Scope::Hour(..) => "←/→: view, ↑/↓: select, Esc: back, ",
    };

    let header = format!(
        "{} — {} ({})   {}   {}q: quit",
        nodename,
        scope_label,
        tabs.join(" "),
        selection,
        nav_hint
    );
    frame.render_widget(Line::from(header), layout[0]);

    let disk_name = m.disk_devices.get(m.disk_idx).map_or("", String::as_str);
    let net_name = m.net_ifaces.get(m.net_idx).map_or("", String::as_str);
    let y_unit = if m.view == View::Network { "kB/s" } else { "%" };

    match &m.scope {
        Scope::All => {
            let (max_s, avg_s, min_s) =
                daily_series(all_samples, days, m.view, disk_name, net_name);
            if avg_s.is_empty() {
                frame.render_widget(
                    Paragraph::new("No data for this view").block(Block::bordered()),
                    layout[1],
                );
                return;
            }

            let y_max = if m.view == View::Network {
                [&max_s, &avg_s, &min_s]
                    .iter()
                    .flat_map(|s| s.iter().map(|(_, y)| *y))
                    .fold(0.0_f64, f64::max)
                    .max(1.0)
            } else {
                100.0
            };

            let n = days.len().max(1);
            let x_bounds = [0.0, (n - 1) as f64];
            let mid = (n - 1) / 2;

            let cursor_data: Vec<(f64, f64)> = avg_s
                .iter()
                .find(|(x, _)| *x as usize == m.day_cursor)
                .copied()
                .into_iter()
                .collect();

            let mut datasets = vec![
                Dataset::default()
                    .name("High")
                    .graph_type(GraphType::Line)
                    .style(Style::default().red())
                    .data(&max_s),
                Dataset::default()
                    .name("Avg")
                    .graph_type(GraphType::Line)
                    .style(Style::default().yellow())
                    .data(&avg_s),
                Dataset::default()
                    .name("Low")
                    .graph_type(GraphType::Line)
                    .style(Style::default().green())
                    .data(&min_s),
            ];
            if !cursor_data.is_empty() {
                datasets.push(
                    Dataset::default()
                        .name("selected")
                        .marker(symbols::Marker::Block)
                        .style(Style::default().white().bold())
                        .data(&cursor_data),
                );
            }

            let x_axis = Axis::default().bounds(x_bounds).labels([
                date_label(days, 0),
                date_label(days, mid),
                date_label(days, n - 1),
            ]);
            let y_axis = Axis::default().title(y_unit).bounds([0.0, y_max]).labels([
                "0".to_string(),
                format!("{:.0}", y_max / 2.0),
                format!("{:.0}", y_max),
            ]);

            let chart = Chart::new(datasets)
                .block(Block::bordered().title(m.view.title()))
                .x_axis(x_axis)
                .y_axis(y_axis);
            frame.render_widget(chart, layout[1]);
        }
        Scope::Day(date) | Scope::Hour(date, _) => {
            let data = series(filtered, m.view, disk_name, net_name);
            if data.is_empty() {
                frame.render_widget(
                    Paragraph::new("No data for this view").block(Block::bordered()),
                    layout[1],
                );
                return;
            }

            let y_max = if m.view == View::Network {
                data.iter()
                    .map(|(_, y)| *y)
                    .fold(0.0_f64, f64::max)
                    .max(1.0)
            } else {
                100.0
            };

            let n = filtered.len().max(1);
            let x_bounds = [0.0, (n - 1) as f64];
            let mid = (n - 1) / 2;

            let dataset = Dataset::default()
                .name(m.view.title())
                .graph_type(GraphType::Line)
                .style(Style::default().cyan())
                .data(&data);

            let x_axis = Axis::default().bounds(x_bounds).labels([
                time_label(filtered, 0, date),
                time_label(filtered, mid, date),
                time_label(filtered, n - 1, date),
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
    }
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

    fn cpu_sample(date: &str, time: &str, usage_percent: f64) -> Sample {
        let mut s = sample(date, time, vec![]);
        s.cpu_load.push(sadf::CpuLoad {
            cpu: "all".to_string(),
            idle: 100.0 - usage_percent,
        });
        s
    }

    #[test]
    fn app_starts_on_the_overview_with_all_samples() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let app = App::new("host".to_string(), samples);
        match app.screen {
            Screen::Metric(ref m) => assert!(matches!(m.scope, Scope::All)),
            _ => panic!("expected the app to start on the Overview (Scope::All)"),
        }
    }

    #[test]
    fn daily_series_computes_max_average_min_per_day() {
        let samples = vec![
            cpu_sample("2026-08-01", "10:00:00", 10.0),
            cpu_sample("2026-08-01", "11:00:00", 30.0),
            cpu_sample("2026-08-02", "10:00:00", 50.0),
        ];
        let days = vec!["2026-08-01".to_string(), "2026-08-02".to_string()];
        let (max_s, avg_s, min_s) = daily_series(&samples, &days, View::Cpu, "", "");
        assert_eq!(max_s, vec![(0.0, 30.0), (1.0, 50.0)]);
        assert_eq!(avg_s, vec![(0.0, 20.0), (1.0, 50.0)]);
        assert_eq!(min_s, vec![(0.0, 10.0), (1.0, 50.0)]);
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
    fn days_are_deduped_and_sorted_chronologically() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-03", "10:00:00", vec![]),
            sample("2026-08-01", "11:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let app = App::new("host".to_string(), samples);
        assert_eq!(app.days, vec!["2026-08-01", "2026-08-02", "2026-08-03"]);
    }

    #[test]
    fn app_starts_with_cursor_on_the_most_recent_day() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-03", "10:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let app = App::new("host".to_string(), samples);
        match app.screen {
            Screen::Metric(ref m) => assert_eq!(
                m.day_cursor, 2,
                "cursor should start on the last (most recent) day"
            ),
            _ => panic!("expected Screen::Metric"),
        }
    }

    #[test]
    fn day_cursor_moves_and_clamps_at_the_edges() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
            sample("2026-08-03", "10:00:00", vec![]),
        ];
        let mut app = App::new("host".to_string(), samples);

        fn cursor(app: &App) -> usize {
            match app.screen {
                Screen::Metric(ref m) => m.day_cursor,
                _ => panic!("expected Screen::Metric"),
            }
        }

        handle_key(&mut app, KeyCode::Left);
        handle_key(&mut app, KeyCode::Left);
        handle_key(&mut app, KeyCode::Left); // should clamp at 0, not go negative
        assert_eq!(cursor(&app), 0);

        handle_key(&mut app, KeyCode::Right);
        handle_key(&mut app, KeyCode::Right);
        handle_key(&mut app, KeyCode::Right); // should clamp at the last day
        assert_eq!(cursor(&app), 2);
    }

    #[test]
    fn enter_on_overview_drills_into_the_day_under_the_cursor() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let mut app = App::new("host".to_string(), samples);
        handle_key(&mut app, KeyCode::Left); // cursor -> day 0 (2026-08-01)
        handle_key(&mut app, KeyCode::Enter);
        match app.screen {
            Screen::Metric(ref m) => {
                assert!(matches!(&m.scope, Scope::Day(d) if d == "2026-08-01"))
            }
            _ => panic!("expected Screen::Metric"),
        }
    }

    #[test]
    fn esc_from_day_view_returns_to_overview_at_that_day() {
        let samples = vec![
            sample("2026-08-01", "10:00:00", vec![]),
            sample("2026-08-02", "10:00:00", vec![]),
        ];
        let mut app = App::new("host".to_string(), samples);
        handle_key(&mut app, KeyCode::Left);
        handle_key(&mut app, KeyCode::Enter);
        handle_key(&mut app, KeyCode::Esc);
        match app.screen {
            Screen::Metric(ref m) => {
                assert!(matches!(m.scope, Scope::All));
                assert_eq!(m.day_cursor, 0);
            }
            _ => panic!("expected Screen::Metric"),
        }
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
            day_cursor: 0,
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
            day_cursor: 0,
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
            day_cursor: 0,
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
