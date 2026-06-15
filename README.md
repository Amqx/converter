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

Licensed under GPL-3.0.