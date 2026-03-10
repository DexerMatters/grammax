use std::path::PathBuf;

use clap::Parser;
use color_print::cprintln;
use grammax::grammar::{Grammar, display::format_grammar_error};

#[derive(clap::Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Compile a grammar file into a binary format
    Compile {
        /// Path to the input grammar file (e.g. my_grammar.gmx)
        #[clap(short, long)]
        input: PathBuf,
        /// Path to the output binary file (e.g. my_grammar.gmx.bin)
        #[clap(short, long)]
        output: Option<PathBuf>,
    },
}

pub fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Compile { input, output }) => {
            let input_path = input;
            let output_path = output.unwrap_or_else(|| input_path.with_extension("bin"));
            let source = std::fs::read_to_string(&input_path).unwrap_or_else(|e| {
                cprintln!(
                    "<red>error:</red> Failed to read input file {}: {}",
                    input_path.display(),
                    e
                );
                std::process::exit(1);
            });
            let source = source.as_str();

            let result = Grammar::interpret(source);
            match result {
                Ok(grammar) => {
                    cprintln!("<green>Grammar compiled successfully.</green>");
                    if let Err(e) = grammar.save_to(&output_path) {
                        cprintln!("<red>error:</red> Failed to save compiled grammar: {}", e);
                    } else {
                        cprintln!("Compiled grammar saved to {}", output_path.display());
                    }
                }
                Err(e) => {
                    cprintln!("<red>error:</red> Failed to compile grammar:");
                    println!("{}", format_grammar_error(&e, source));
                }
            }
        }
        None => {
            println!("No command provided. Use --help for usage information.");
        }
    }
}
