use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use mcp_brain::{
    server::{Brain, pretty},
    vault::Vault,
};
use rmcp::{ServiceExt, transport::stdio};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "Ownership-aware MCP memory for a local Markdown PARA vault",
    author
)]
struct Cli {
    /// Existing vault directory. Use an absolute path in MCP client configuration.
    #[arg(long)]
    vault: PathBuf,
    /// Local state directory outside the vault. All clients for a vault must share it.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve MCP over stdin/stdout. Diagnostics go to stderr.
    Serve,
    /// Show PARA counts, vault instructions, and Inbox status.
    Status,
    /// Review captures and locally approve proposed moves.
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
    /// Configure opt-in reminders, check due status, or export a calendar reminder.
    Reminder {
        #[command(subcommand)]
        command: ReminderCommand,
    },
}

#[derive(Subcommand)]
enum InboxCommand {
    Status,
    Review {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the complete local proposal, including before/after content.
    Show {
        id: String,
    },
    /// Display the proposal and require confirmation from an interactive terminal.
    Apply {
        id: String,
    },
    /// Record a manual review even when no notes needed moving.
    MarkReviewed,
}

#[derive(Subcommand)]
enum ReminderCommand {
    Configure {
        #[arg(long)]
        every_days: u32,
    },
    Disable,
    /// Print a message only when review is due. --exit-code returns 3 when due.
    Check {
        #[arg(long)]
        exit_code: bool,
    },
    /// Export a recurring .ics reminder to stdout; import it in a calendar app.
    Calendar {
        #[arg(long, default_value = "18:00")]
        time: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let vault = Vault::open(cli.vault, cli.state_dir)?;
    match cli.command {
        Command::Serve => {
            Brain::new(vault).serve(stdio()).await?.waiting().await?;
        }
        Command::Status => println!("{}", pretty(&vault.context()?)?),
        Command::Inbox { command } => match command {
            InboxCommand::Status => println!("{}", pretty(&vault.inbox_status()?)?),
            InboxCommand::Review { offset, limit } => {
                println!("{}", pretty(&vault.inbox_review(offset, limit)?)?)
            }
            InboxCommand::Show { id } => println!("{}", pretty(&vault.load_plan(&id)?)?),
            InboxCommand::Apply { id } => {
                ensure!(
                    io::stdin().is_terminal(),
                    "approval requires an interactive terminal; inspect the plan and run this command locally"
                );
                let plan = vault.load_plan(&id)?;
                println!("{}", pretty(&plan)?);
                print!("\nApply these exact changes? Type the plan ID {id} to confirm: ");
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                ensure!(answer.trim() == id, "cancelled; no notes changed");
                println!("{}", pretty(&vault.apply_plan(&id)?)?);
            }
            InboxCommand::MarkReviewed => println!("{}", pretty(&vault.mark_reviewed()?)?),
        },
        Command::Reminder { command } => match command {
            ReminderCommand::Configure { every_days } => {
                println!("{}", pretty(&vault.configure_reminder(Some(every_days))?)?)
            }
            ReminderCommand::Disable => println!("{}", pretty(&vault.configure_reminder(None)?)?),
            ReminderCommand::Check { exit_code } => {
                let status = vault.inbox_status()?;
                if status["due"] == true {
                    println!(
                        "Inbox review is due: {} captures. Run mcp-brain --vault <path> inbox review.",
                        status["count"]
                    );
                    if exit_code {
                        std::process::exit(3);
                    }
                }
            }
            ReminderCommand::Calendar { time } => {
                let status = vault.inbox_status()?;
                let days = status["reminder"]["every_days"]
                    .as_u64()
                    .context("configure a reminder before exporting a calendar")?;
                let time = chrono::NaiveTime::parse_from_str(&time, "%H:%M")
                    .context("time must be HH:MM")?;
                let start = (chrono::Local::now().date_naive() + chrono::Duration::days(1))
                    .and_time(time)
                    .format("%Y%m%dT%H%M%S")
                    .to_string();
                // Floating local time deliberately follows the calendar user's zone.
                let lines = [
                    "BEGIN:VCALENDAR".into(),
                    "VERSION:2.0".into(),
                    "PRODID:-//mcp-brain//Inbox Review//EN".into(),
                    "BEGIN:VEVENT".into(),
                    format!("UID:{}@mcp-brain", uuid::Uuid::new_v4()),
                    format!("DTSTAMP:{}", chrono::Utc::now().format("%Y%m%dT%H%M%SZ")),
                    format!("DTSTART:{start}"),
                    "DURATION:PT15M".into(),
                    format!("RRULE:FREQ=DAILY;INTERVAL={days}"),
                    "SUMMARY:Review your notes inbox".into(),
                    "DESCRIPTION:Choose destinations for your unsorted captures.".into(),
                    "BEGIN:VALARM".into(),
                    "TRIGGER:PT0S".into(),
                    "ACTION:DISPLAY".into(),
                    "DESCRIPTION:Review your notes inbox".into(),
                    "END:VALARM".into(),
                    "END:VEVENT".into(),
                    "END:VCALENDAR".into(),
                ];
                print!("{}\r\n", lines.join("\r\n"));
            }
        },
    }
    Ok(())
}
