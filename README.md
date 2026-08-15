# sysplot

TUI viewer for sysstat (`.sar`) files, powered by `sadf`.

Reads a `sysstat` binary data file (e.g. `/var/log/sysstat/sa15`) via
`sadf -j -- -A` and renders CPU, memory, disk, and network activity as
terminal graphs you can page through interactively.

## Install

- Cargo: `cargo install sysplot`
- Debian package: https://github.com/wabuntu/sysplot/tree/main/target/debian
- RPM package: https://github.com/wabuntu/sysplot/tree/main/target/release/rpmbuild/RPMS/x86_64
- Single binary: https://github.com/wabuntu/sysplot/tree/main/binaries

Requires `sysstat` to be installed (for `sadf`).

## Usage

```
$ sysplot /var/log/sysstat/sa15
```

Keys:

- `←`/`→` (or Tab/Shift+Tab): switch between CPU / Memory / Disk / Network
- `↑`/`↓`: cycle the selected disk device / network interface
- `q` / `Esc`: quit
