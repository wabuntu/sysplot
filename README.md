# sysplot

TUI viewer for sysstat (`.sar`) history, powered by `sadf`.

Reads every sysstat data file in a directory (e.g. `/var/log/sysstat`) via
`sadf -j -- -A`, in parallel, and lets you browse the last 30 days of CPU,
memory, disk, and network activity — drill from a day down to a single hour
without leaving the terminal.

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

1. **Days** — the last 30 days that have data, most recent first.
2. **Day view** — `←`/`→` switches CPU / Memory / Disk / Network, `↑`/`↓`
   cycles the selected disk device / network interface.
3. **Hours** — press `Enter` from a day view to list the hours it has
   samples for.
4. **Hour view** — same CPU/Memory/Disk/Network controls, zoomed to that
   hour's samples.

`Esc` goes back one level, `q` quits from anywhere.

Note: the finest resolution shown is whatever your sysstat collection
interval actually is (commonly every 10 minutes) — drilling into an hour
zooms into that hour's real samples, it doesn't invent per-minute data that
was never collected.
