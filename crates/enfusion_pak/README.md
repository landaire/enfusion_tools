# enfusion_pak

A library for reading Enfusion game engine `.pak` files.

## Usage

For the CLI tool:

```sh
$ cargo install --locked enfusion_pak
$ enfusion_pak --help

A library/cli for reading Enfusion game engine `.pak` files.

Usage: enfusion_pak [OPTIONS] <FILE>

Arguments:
  <FILE>  Path to either a single file or a directory containing `.pak` files

Options:
  -l, --long     Print long file information
  -m, --merged   Virtually merge contents of files together so that duplicate directories across multiple `.pak` files are treated as a single filesystem entry
  -h, --help     Print help
  -V, --version  Print version
```


For the library:

```sh
cargo add enfusion_pak
```

## Support

This currently supports PAK files versioned at `0x10003`. Currently older versions are not supported (although they wouldn't be difficult to add if needed).

## Features

- sans-io core parser with out-of-the-box support for sync callers. Async wouldn't be too hard to add.
- VFS support through the [`vfs`](https://docs.rs/vfs/latest/vfs/) crate.
- Performant file reading operations

## PAK Format

The PAK format is somewhat similar to other container file formats like MP4, but does have inter-chunk references.

The main parsing logic can be found in [`src/parser.rs`](src/parser.rs).

The following diagram describes the general format:

```text
    Chunk format:

┌─────────────────────┬────────────────────┬──────────────────────────────┐
│  4 byte identifier  │  4 byte data len   │  Identifier-specific data... │
└─────────────────────┴────────────────────┴──────────────────────────────┘

                  ┌───────────────────────┐
                  │      Header Chunk     │
                  ├───────────────────────│
                  │                       │
                  │   Additional Chunks   │
                  │                       │
                  │                       │
                  ├───────────────────────│
                  │                       │
                  │      Data Chunk       │
                  │                       │
                  │                       │
                  │                       │
                  │    Casual 1.8GiB      │
               ┌─▶│       of data         │◀─┐
               │  │                       │  │┌───────────┐
               │  │                       │  ││ File Meta │
               │  │                       │  ││has offset │
               │  ├───────────────────────┤  ││ into data │
               │  │      File Chunk       │  ││   chunk   │
               │  │                       │  ││           │
               │  ├───────────┬───────────┤  │└───────────┘
               │  │ File Meta │ File Meta │──┘
               │  ├───────────┼───────────┤
               └──│ File Meta │ File Meta │
                  ├───────────┼───────────┤
                  │ File Meta │ File Meta │
                  └───────────┴───────────┘
```
