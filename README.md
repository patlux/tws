# tws

tmux workspace manager — organize sessions into collections and threads.

tws is a TUI that adds an organizational layer on top of tmux sessions. Group your sessions into **threads**, and threads into **collections**.

- **Collections** — top-level groups (e.g. "work", "personal", "infra")
- **Threads** — within a collection, each with one or more tmux sessions
- **Sessions** — ephemeral tmux sessions, launched and managed from tws

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/ytaskiran/tws/main/install.sh | bash
```

This downloads the latest release binary to `~/.local/bin`.

Supports macOS and Linux, both x86_64 and ARM.

### Upgrade

Run the same command again. It fetches the latest release and replaces the binary.

## Usage

Run `tws` in a terminal. Use `prefix + d` to detach from a session and return to your shell.

## License

[MIT](LICENSE)
