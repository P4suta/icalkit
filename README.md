# Icalkit

A Rust library for parsing, editing, and querying iCalendar data.

## Installation

```sh
cargo add icalkit --git https://github.com/P4suta/icalkit.git
```

## Usage

```rust
fn print_event_ids(bytes: &[u8]) -> Result<(), icalkit::Error> {
    let calendar = icalkit::Calendar::parse(bytes)?;
    for event in calendar.events() {
        if let Some(uid) = event.uid() {
            println!("{uid}");
        }
    }
    Ok(())
}
```

## Documentation

[Read the documentation](docs/README.md).

## License

Licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the [Massachusetts Institute of Technology (MIT) license](LICENSE-MIT), at your option.

Contributions use both licenses unless you state other terms.
