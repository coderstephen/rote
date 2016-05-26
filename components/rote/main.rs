extern crate filetime;
extern crate getopts;
extern crate glob;
extern crate lazysort;
#[macro_use] extern crate log;
extern crate regex;
extern crate runtime;
extern crate term;

mod logger;
mod runner;
mod stdlib;
mod graph;

use getopts::Options;
use lazysort::SortedBy;
use runtime::Environment;
use std::env;
use std::path;
use std::process;


// Define some global constants for various metadata.
const ROTE_TITLE: &'static str = "
   ▄████████  ▄██████▄      ███        ▄████████
  ███    ███ ███    ███ ▀█████████▄   ███    ███
  ███    ███ ███    ███    ▀███▀▀██   ███    █▀
 ▄███▄▄▄▄██▀ ███    ███     ███   ▀  ▄███▄▄▄
▀▀███▀▀▀▀▀   ███    ███     ███     ▀▀███▀▀▀
▀███████████ ███    ███     ███       ███    █▄
  ███    ███ ███    ███     ███       ███    ███
  ███    ███  ▀██████▀     ▄████▀     ██████████
  ███    ███\n";

const ROTE_VERSION: &'static str = env!("CARGO_PKG_VERSION");


/// Prints the program usage to the console.
fn print_usage(options: Options) {
    let short_usage = options.short_usage("rote");

    print!("{}\r\n{}", ROTE_TITLE, options.usage(&short_usage));
}

fn print_task_list(runner: &runner::Runner) {
    let mut out = term::stdout().unwrap();

    println!("Available tasks:");

    for task in runner.tasks.iter().sorted_by(|a, b| {
        a.0.cmp(b.0)
    }) {
        out.fg(term::color::BRIGHT_GREEN).unwrap();
        write!(out, "  {:16}", task.0).unwrap();
        out.reset().unwrap();

        if let Some(ref description) = task.1.description {
            write!(out, "{}", description).unwrap();
        }

        writeln!(out, "").unwrap();
    }

    if let Some(ref default) = runner.default_task() {
        println!("");
        println!("Default task: {}", default.name);
    }
}

/// Parses command-line options and runs retest.
fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse command-line flags.
    let mut options = Options::new();
    options.optopt("C", "directory", "Change to DIRECTORY before running tasks.", "DIRECTORY");
    options.optflag("d", "dry-run", "Don't actually perform any action.");
    options.optopt("f", "file", "Read FILE as the Rotefile.", "FILE");
    options.optflag("h", "help", "Print this help menu and exit.");
    options.optopt("j", "jobs", "The number of jobs to run simultaneously.", "N");
    options.optflag("l", "list", "List available tasks.");
    options.optflag("q", "quiet", "Supress all non-task output.");
    options.optflag("V", "version", "Print the program version and exit.");
    options.optflagmulti("v", "verbose", "Enable verbose logging.");

    let matches = options.parse(&args[1..]).unwrap_or_else(|err| {
        logger::init(logger::Filter::Error).unwrap();
        error!("{}", err);
        process::exit(2);
    });

    // Set the logging verbosity level.
    logger::init(if matches.opt_present("quiet") {
        logger::Filter::Error
    } else {
        match matches.opt_count("verbose") {
            0 => logger::Filter::Warn,
            1 => logger::Filter::Info,
            2 => logger::Filter::Debug,
            _ => logger::Filter::Trace,
        }
    }).unwrap();

    // Notify the user if higher vebosity has been achieved.
    debug!("debug messages turned on");
    trace!("trace messages turned on");

    // If the help flag is present show the usage message.
    if matches.opt_present("help") {
        print_usage(options);
        return;
    }

    // If the version flag is present, show the program version.
    if matches.opt_present("version") {
        println!("Rote version {}", ROTE_VERSION);
        return;
    }

    // If the directory flag is present, change directories first.
    if let Some(directory) = matches.opt_str("directory") {
        if env::set_current_dir(&directory).is_err() {
            error!("failed to change directory to '{}'", &directory);
            process::exit(1);
        }
    }

    // Get the full path of the Rotefile to run.
    let filename = matches.opt_str("file").unwrap_or("Rotefile".to_string());
    let path = path::Path::new(&filename)
        .canonicalize()
        .unwrap_or_else(|_| {
            error!("the path '{}' is not a file or is not readable", filename);
            process::exit(1);
        });

    // Set the new current directory to the directory containing the Rotefile.
    if let Some(directory) = path.parent() {
        if env::set_current_dir(&directory).is_err() {
            error!("failed to change directory to '{}'", &directory.to_string_lossy());
            process::exit(1);
        }
    }

    // Set up the environment.
    let environment = Environment::new(path, matches.opt_present("dry-run"));

    info!("build file: {}", environment.path().to_str().unwrap());

    // Create a new script runtime.
    let mut runner = runner::Runner::new().unwrap_or_else(|e| {
        error!("{}", e);
        process::exit(1);
    });
    if let Err(e) = runner.load(environment.path()) {
        error!("{}", e);
        process::exit(1);
    }

    // List all tasks instead of running one.
    if matches.opt_present("list") {
        print_task_list(&runner);
        return;
    }

    // Get all of the task arguments.
    let mut args = matches.free.clone();

    // Run the specified task, or the default if none is specified.
    if let Err(e) = {
        if args.is_empty() {
            runner.run_default()
        } else {
            // Run the specified task.
            let task_name = args.remove(0);
            runner.run(&task_name)
        }
    } {
        error!("{}", e);
        process::exit(1);
    }
}
