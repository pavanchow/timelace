use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use timelace::commands;
use timelace::store::{Store, StoreError};
use timelace::{Object, ObjectId};

#[derive(Parser)]
#[command(name = "timelace", about = "A tiny content-addressed version control system")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new repository in the current directory
    Init,
    /// Stage a file or directory for the next commit
    Add { path: PathBuf },
    /// Snapshot the staged tree as a new commit
    Commit {
        #[arg(short, long)]
        message: String,
    },
    /// Show commit history starting from HEAD
    Log,
    /// Show staged vs untracked files
    Status,
    /// Restore the working tree to a given commit
    Checkout { commit: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot read current directory: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command {
        Command::Init => commands::init(&cwd),
        Command::Add { path } => Store::discover(&cwd).and_then(|s| commands::add(&s, &path)),
        Command::Commit { message } => Store::discover(&cwd).and_then(|s| {
            commands::commit(&s, &message).map(|id| println!("committed {id}"))
        }),
        Command::Log => Store::discover(&cwd).and_then(|s| {
            commands::log(&s).map(|entries| print_log(&entries))
        }),
        Command::Status => Store::discover(&cwd).and_then(|s| {
            commands::status(&s).map(|entries| print_status(&entries))
        }),
        Command::Checkout { commit } => Store::discover(&cwd).and_then(|s| {
            ObjectId::parse(&commit)
                .map_err(StoreError::Corrupt)
                .and_then(|id| commands::checkout(&s, &id))
        }),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_log(entries: &[(ObjectId, Object)]) {
    for (id, obj) in entries {
        if let Object::Commit {
            message, timestamp, ..
        } = obj
        {
            println!("commit {id}");
            println!("timestamp {timestamp}");
            println!("\n    {message}\n");
        }
    }
}

fn print_status(entries: &[commands::StatusEntry]) {
    for e in entries {
        match e {
            commands::StatusEntry::Staged(p) => println!("staged:     {p}"),
            commands::StatusEntry::Untracked(p) => println!("untracked:  {p}"),
        }
    }
}
