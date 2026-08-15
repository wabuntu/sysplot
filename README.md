# sysplot

TUI viewer for sysstat (`.sar`) history, powered by `sadf`.

Reads every sysstat data file in a directory (e.g. `/var/log/sysstat`) via
`sadf -j -- -A`, in parallel, and lets you browse the last 30 days of CPU,
memory, disk, and network activity — drill from a day down to a single hour
without leaving the terminal.

<img src="https://raw.githubusercontent.com/wabuntu/sysplot/main/docs/overview.png" alt="sysplot Overview screen: CPU/Memory/Disk/Network with max/average/min lines across the last 8 days" width="570">
<img src="https://raw.githubusercontent.com/wabuntu/sysplot/main/docs/day.png" alt="sysplot day view: a single day's network throughput on a Wi-Fi interface" width="570">

## Install

- Cargo: `cargo install sysplot`
- Debian package: https://github.com/wabuntu/sysplot/tree/main/target/debian
- RPM package: https://github.com/wabuntu/sysplot/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/sysplot/tree/main/binaries

Requires `sysstat` to be installed (for `sadf`).

## Usage

```
$ sysplot                          # defaults to /var/log/sysstat
$ sysplot /var/log/sysstat         # a directory of saXX/saYYYYMMDD files
$ sysplot /var/log/sysstat/sa15    # a single file
```

Navigation:

1. **Overview** (the screen you land on) — CPU / Memory / Disk / Network,
   one point per day across the whole loaded range, plotted as max/average/min
   lines. `←`/`→` switches metric, `↑`/`↓` cycles the selected disk device /
   network interface.
2. **Days** — press `Enter` from the overview to list the last 30 days that
   have data, most recent first.
3. **Day view** — same CPU/Memory/Disk/Network controls, zoomed to a single
   day's raw samples.
4. **Hours** — press `Enter` from a day view to list the hours it has
   samples for.
5. **Hour view** — same controls again, zoomed to that hour's samples.

`Esc` goes back one level, `q` quits from anywhere.

Note: the finest resolution shown is whatever your sysstat collection
interval actually is (commonly every 10 minutes) — drilling into an hour
zooms into that hour's real samples, it doesn't invent per-minute data that
was never collected.
