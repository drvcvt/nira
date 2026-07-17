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
