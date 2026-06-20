# Converter

Converts reader backup formats from one to another.

Currently supported:
- [TachiyomiSy](https://github.com/jobobby04/TachiyomiSY)
- [Aidoku](https://aidoku.app/)

## Install

```shell
cargo install --git https://github.com/amqx/converter.git
converter -h
```

## Build from source

```shell
git clone https://github.com/amqx/converter.git && cd converter

# Run debug
cargo run -- -h

# Install release
cargo install --path .
converter -h

```
## Developing

The project contains two helper scripts for decoding Aidoku and TachiyomiSY's backups. These live in the `scripts/`
directory. Install dependencies with [uv](https://docs.astral.sh/uv/):

```shell
# Make sure you have uv installed beforehand
uv sync

uv run ./scripts/decode_tachiyomi.py <backup.tachibk>
uv run ./scripts/decode_aidoku.py <backup.aib>
```

## Notes
- Aidoku needs a chance to update all imported library entries before correctly registering finished chapter counts.

Licensed under GPL-3.0.
