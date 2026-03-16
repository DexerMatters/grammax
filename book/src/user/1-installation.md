# Installation

Grammax is both a command-line tool and a library which you can add to your project. 

## Pre-compiled binaries

> [!WARNING]
> In progress...

## Build from source

To build Grammax from source, you will first need to install Rust and Cargo. Please follow the instructions on the [Rust installation page](https://rust-lang.org/tools/install/).

### Install CLI

After you install Rust and Cargo, you can install Grammax with the following command:
```bash
cargo install grammax --all-features
```
This will automatically download Grammax from [crates.io](https://crates.io/) and install the executable file into Cargo's global binary directory (by default, `~/.cargo/bin` on Linux or `%USERPROFILE%\.cargo\bin` on Windows). Make sure there is the directory in the environment variable `$PATH`.

Then run the following command in your terminal to verify the installation. It prints the version of Grammax if installation succeeds.
```bash
gmx --version
```

You can update Grammax by repeating the installation command `cargo install grammax`. The command will check if there is a newer version and reinstall Grammax if the newer one is found.

To uninstall Grammax, you can run the following command:

```bash
cargo uninstall grammax
```

### Add to your project

You can add Grammax as a dependency to your Rust project by running the following command at the root of your project:

```bash
cargo add grammax
```

Grammax has a lot of [features](). Append the flag `--features` to the command once you want to add the dependency with any features. For example, if you want to enable the web preview interface, you can add Grammax with the following command:

```bash
cargo add grammax --features "webui"
```

Or, you can open `cargo.toml` in your project and add the following line to the end of the `[[dependencies]]`. For example:

```toml
[dependencies]
...
grammax = { version = "0.1", features = ["webui"] }
```

For more details, read the chapter [specifying dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html) in the [Cargo book](https://doc.rust-lang.org/cargo/index.html).