# AGENTS.md — Nira

Use the named tasks in `anvil.toml` for resource-heavy work:

```sh
anvil tests
anvil check
anvil dev
anvil release
```

Do not run the equivalent Cargo or Dioxus build locally unless the user asks.
Worker inventory and machine-wide resource policy belong in the global Anvil
config, not this repository. All named tasks run inside the project's
`shell.nix`; add native build dependencies there instead of embedding
`nix-shell -p` package lists in individual tasks.

## Repository safety

Before changing files, run `git branch --show-current`, `git status --short`,
and `git worktree list`.

- `/home/mt/projects/nira` stays on public-safe `public`.
- `/home/mt/projects/nira-private` stays on private `master`.
- Never merge or copy Qobuz code, private configuration, credentials, or
  provider wiring into `public`.
- Make shared changes on `public`, verify and commit them there, then
  cherry-pick only those exact commits into `master`, preserving Qobuz when
  resolving conflicts.
- The installed launcher is private and must point at the release built from
  `/home/mt/projects/nira-private`.
- Before pushing `public`, verify a clean worktree and audit the diff for
  private code, paths, credentials, and Qobuz wiring. Never push `master`
  unless the user explicitly asks.

## User data and input safety

- Never replace `likes.json`, `playlists.json`, or other durable user data
  with empty state after a parse error. Snapshot the files before recovery or
  migration; retired provider IDs must deserialize as `Unavailable`.
- Never automate or take control of the user's mouse or keyboard with
  `ydotool`, `wtype`, `xdotool`, or similar tools.
- Preserve unrelated user changes and avoid destructive Git commands.
