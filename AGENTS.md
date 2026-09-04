# AGENTS.md

## Toolchain

- Before running any Rust or Cargo command in this repository, run `vfox use -p rust@1.98.0`.
- Keep every Cargo package at `rust-version = "1.98"`.

## Commits and releases

Every commit must use Conventional Commits in the literal format `<type>[optional scope][!]: <description>`. `feat`, `fix`, `perf`, and `revert` generate releases; `!` or a `BREAKING CHANGE` footer generates a major release. `docs`, `chore`, `ci`, `test`, `refactor`, `style`, and `build` do not generate a release when there is no breaking change.
