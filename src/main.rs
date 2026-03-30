use std::path::PathBuf;

use clap::{Args, Parser};
use color_print::cprintln;
use grammax::{
    grammar::{Grammar, display::format_grammar_error},
    interface::cli::CliInterface,
    runtime::{Build, ParserPass},
    scheme::layers::{ParseTreeIR, SourceText},
};

#[cfg(feature = "webui")]
use grammax::interface::webui::WebPreviewInterface;

#[derive(clap::Parser)]
#[command(version, about, long_about = None, name="gmx")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Compile a grammar file into a binary format
    Compile {
        /// Path to the input grammar file (e.g. my_grammar.gmx)
        input: PathBuf,
        /// Path to the output binary file (e.g. my_grammar.gmx.bin)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Interactively test a grammar against input strings
    Test {
        /// Path to the compiled grammar file (e.g. my_grammar.gmx.bin)
        grammar: PathBuf,

        #[allow(dead_code)]
        #[command(flatten)]
        interactive_mode: InteractiveMode,
    },
}

#[derive(Args)]
#[group(multiple = false)]
struct InteractiveMode {
    /// Use a simple command-line interface for testing
    #[arg(long)]
    cli: bool,

    /// Use a web-based interface for testing
    #[cfg(feature = "webui")]
    #[arg(long)]
    webui: bool,
}

pub fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Compile { input, output }) => {
            let input_path = input;
            let output_path = output.unwrap_or_else(|| input_path.with_extension("gmx.bin"));
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
                    if output_path.exists() {
                        cprintln!(
                            "<yellow>warning:</yellow> Output file {} already exists. Do you want to overwrite it? <bold>(y/N)</bold>",
                            output_path.display()
                        );
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input).unwrap();
                        if input.trim().to_lowercase() != "y" {
                            cprintln!("Aborting.");
                            std::process::exit(0);
                        }
                    }
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
        Some(Commands::Test {
            grammar,
            interactive_mode,
        }) => {
            #[cfg(not(feature = "webui"))]
            let _ = &interactive_mode;

            let grammar = match Grammar::load_from(&grammar) {
                Ok(g) => g,
                Err(e) => {
                    cprintln!("<red>error:</red> Failed to load grammar binary: \n{}", e);
                    std::process::exit(1);
                }
            };
            let tree = Build::new().then(
                || ParserPass::new(grammar),
                ParseTreeIR::with_grammar(grammar),
                |b, _| b,
            );
            #[cfg(feature = "webui")]
            if interactive_mode.webui {
                start_webui(tree, grammar);
                return;
            }
            start_cli(tree, grammar);
        }
        None => {
            cprintln!("No command provided. Use '<bold>--help</bold>' for usage information.");
        }
    }
}

use grammax::runtime::{End, Then};

type ParseTreePass = Then<SourceText, ParserPass, End<ParseTreeIR>>;

fn start_cli(tree: Build<ParseTreePass>, grammar: &'static Grammar) {
    tree.build_runtime::<CliInterface<ParseTreePass>>(grammar)
        .run()
        .unwrap_or_else(|e| {
            cprintln!("<red>error:</red> Runtime failed unexpectedly: \n{}", e);
            std::process::exit(1);
        })
}

#[cfg(feature = "webui")]
fn start_webui(tree: Build<ParseTreePass>, grammar: &'static Grammar) {
    tree.build_runtime::<WebPreviewInterface<ParseTreePass>>(grammar)
        .run()
        .unwrap_or_else(|e| {
            cprintln!("<red>error:</red> Runtime failed unexpectedly: \n{}", e);
            std::process::exit(1);
        });
}
