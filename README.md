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

## Notes
- Aidoku needs a chance to update all imported library entries before correctly registering finished chapter counts.

Licensed under GPL-3.0.
