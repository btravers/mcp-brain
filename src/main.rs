use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use mcp_brain::{server::Brain, vault::Vault};

mod picker;
mod screen;
mod terminal;
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
    /// Emit JSON for scripts and LLMs (MCP serve is always structured).
    #[arg(long, global = true)]
    json: bool,
    /// Color in human-readable output. Auto respects NO_COLOR and TERM=dumb.
    #[arg(long, global = true, value_enum, default_value_t = Color::Auto)]
    color: Color,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
enum Command {
    /// Serve MCP over stdin/stdout. Diagnostics go to stderr.
    Serve,
    /// Show PARA counts, vault instructions, and Inbox status.
    Status,
    /// Create missing PARA directories and print portable client configuration examples.
    Init,
    /// Check local setup without contacting a remote or changing notes.
    Doctor,
    /// Fetch and fast-forward a clean Git vault, then push local commits.
    Sync,
    /// Show synchronization freshness, policy, and pending work.
    SyncStatus,
    /// Opt-in foreground checkpointing of human saves. Stop with Ctrl-C.
    Watch {
        #[arg(long, default_value_t = 10)]
        debounce_seconds: u64,
        /// Checkpoint and push once; do not start a background loop.
        #[arg(long)]
        once: bool,
    },
    /// Local-only policy. Omitted settings retain their existing values.
    Config {
        #[arg(long, action = clap::ArgAction::Set)]
        offline_writes: Option<bool>,
        #[arg(long, action = clap::ArgAction::Set)]
        sync_on_start: Option<bool>,
        /// Minimum 30 seconds; 0 disables periodic sync.
        #[arg(long)]
        sync_interval_seconds: Option<u64>,
    },
    Recover {
        #[command(subcommand)]
        command: RecoveryCommand,
    },
    /// Rebuild the private local full-text index from Markdown.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Search words in note paths and full content, ranked by distinct matches.
    Search {
        query: String,
        #[arg(long)]
        category: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Review overdue memories, changed sources, and lexical conflict candidates.
    MemoryReview {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
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
enum IndexCommand {
    Rebuild,
}

#[derive(Subcommand)]
enum RecoveryCommand {
    /// Retry only the pending upload; never repeat a note mutation.
    Push,
    /// Display exact before/after text from a local transaction journal.
    Show { journal: String },
    /// Resume an interrupted transaction after interactive approval.
    Finish { journal: String },
}

fn approve(id: &str) -> Result<()> {
    ensure!(
        io::stdin().is_terminal(),
        "approval requires an interactive terminal"
    );
    eprint!("\nApply these exact changes? Type {id} to confirm: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    ensure!(answer.trim() == id, "cancelled; no notes changed");
    Ok(())
}

#[derive(Subcommand)]
enum InboxCommand {
    Status,
    /// List recent saved proposals, their note names, progress and status.
    Plans {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Review {
        /// Review this plan; omit to choose a saved plan in an interactive terminal.
        id: Option<String>,
        /// Use the line-oriented batch reviewer instead of the full-screen UI.
        #[arg(long, requires = "id")]
        plain: bool,
        #[arg(long, default_value_t = 0, conflicts_with = "id")]
        offset: usize,
        #[arg(long, default_value_t = 20, conflicts_with = "id")]
        limit: usize,
    },
    /// Show the complete local proposal, including before/after content.
    Show {
        id: String,
        /// Include exact before/after text for every affected file.
        #[arg(long)]
        details: bool,
    },
    /// Read saved decisions and comments, including after a plan expires.
    Feedback {
        id: String,
    },
    /// Display the proposal and require confirmation from an interactive terminal.
    Apply {
        id: String,
    },
    /// Preview and locally confirm Undo of the last individually applied move.
    Undo {
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
    let style = terminal::Style {
        color: match cli.color {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => {
                io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").as_deref() != Ok("dumb")
            }
        },
    };
    macro_rules! emit {
        ($value:expr) => {
            terminal::value(&mut io::stdout().lock(), &$value, cli.json, style)?
        };
    }
    if matches!(cli.command, Command::Init) {
        std::fs::create_dir_all(&cli.vault)?;
    }
    let vault = Vault::open(cli.vault, cli.state_dir)?;
    match cli.command {
        Command::Serve => {
            let policy = vault.policy()?;
            if policy.sync_on_start {
                let v = vault.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || v.synchronize()).await? {
                    eprintln!("startup sync: {e:#}");
                }
            }
            let background = policy.sync_interval_seconds.map(|seconds| {
                let v = vault.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(seconds.max(30))).await;
                        let v = v.clone();
                        match tokio::task::spawn_blocking(move || v.synchronize()).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => eprintln!("periodic sync: {e:#}"),
                            Err(e) => eprintln!("periodic sync: {e}"),
                        }
                    }
                })
            });
            Brain::new(vault).serve(stdio()).await?.waiting().await?;
            if let Some(task) = background {
                task.abort();
            }
        }
        Command::Status => emit!(vault.context()?),
        Command::Init => emit!(vault.initialize()?),
        Command::Doctor => {
            let result = vault.doctor()?;
            emit!(result);
            if result["ok"] != true {
                std::process::exit(1);
            }
        }
        Command::Sync => emit!(vault.synchronize()?),
        Command::SyncStatus => emit!(vault.sync_status()?),
        Command::Watch {
            debounce_seconds,
            once,
        } => {
            if once {
                emit!(vault.watch_once()?);
            } else {
                vault.watch(debounce_seconds)?;
            }
        }
        Command::Config {
            offline_writes,
            sync_on_start,
            sync_interval_seconds,
        } => {
            let mut policy = vault.policy()?;
            if let Some(v) = offline_writes {
                policy.offline_writes = v;
            }
            if let Some(v) = sync_on_start {
                policy.sync_on_start = v;
            }
            if let Some(v) = sync_interval_seconds {
                policy.sync_interval_seconds = (v != 0).then_some(v);
            }
            if offline_writes.is_some()
                || sync_on_start.is_some()
                || sync_interval_seconds.is_some()
            {
                emit!(vault.configure(policy)?);
            } else {
                emit!(policy);
            }
        }
        Command::Recover { command } => match command {
            RecoveryCommand::Push => emit!(vault.retry_push()?),
            RecoveryCommand::Show { journal } => {
                let changes = vault.recovery_show(&journal)?;
                if cli.json {
                    emit!(changes);
                } else {
                    terminal::changes(&mut io::stdout().lock(), &changes, style)?;
                }
            }
            RecoveryCommand::Finish { journal } => {
                ensure!(
                    io::stdin().is_terminal(),
                    "recovery approval requires an interactive terminal"
                );
                terminal::changes(
                    &mut io::stderr().lock(),
                    &vault.recovery_show(&journal)?,
                    style,
                )?;
                approve(&journal)?;
                emit!(vault.recovery_finish(&journal)?);
            }
        },
        Command::Index {
            command: IndexCommand::Rebuild,
        } => emit!(vault.rebuild_index()?),
        Command::Search {
            query,
            category,
            offset,
            limit,
        } => emit!(vault.search_notes(&query, category.as_deref(), offset, limit)?),
        Command::MemoryReview { offset, limit } => {
            emit!(vault.memory_review(offset, limit)?);
        }
        Command::Inbox { command } => match command {
            InboxCommand::Status => emit!(vault.inbox_status()?),
            InboxCommand::Plans { limit } => terminal::plan_listing(
                &mut io::stdout().lock(),
                &vault.inbox_plans(limit)?,
                cli.json,
                style,
            )?,
            InboxCommand::Review {
                id: Some(id),
                plain,
                ..
            } => {
                ensure!(
                    !cli.json,
                    "interactive review cannot use --json; use inbox feedback PLAN_ID --json"
                );
                ensure!(
                    io::stdin().is_terminal() && io::stdout().is_terminal(),
                    "review requires an interactive terminal; use inbox show PLAN_ID or --json for scripts"
                );
                if plain {
                    terminal::review(
                        &vault,
                        &id,
                        &mut io::stdin().lock(),
                        &mut io::stdout().lock(),
                        style,
                    )?;
                } else {
                    screen::run(vault.clone(), &id, style.color)?;
                }
            }
            InboxCommand::Review {
                id: None,
                offset,
                limit,
                ..
            } => {
                if !cli.json
                    && offset == 0
                    && io::stdin().is_terminal()
                    && io::stdout().is_terminal()
                {
                    if std::env::var("TERM").as_deref() == Ok("dumb") {
                        terminal::plan_listing(
                            &mut io::stdout().lock(),
                            &vault.inbox_plans(limit)?,
                            false,
                            style,
                        )?;
                    } else if let Some(id) = picker::run(&vault, style.color, limit)? {
                        screen::run(vault.clone(), &id, style.color)?;
                    }
                } else {
                    emit!(vault.inbox_review(offset, limit)?);
                }
            }
            InboxCommand::Feedback { id } => emit!(vault.inbox_plan_review(&id)?),
            InboxCommand::Show { id, details } => {
                let plan = vault.load_plan(&id)?;
                if cli.json {
                    emit!(plan);
                } else {
                    terminal::plan(
                        &mut io::stdout().lock(),
                        &plan,
                        Some(&vault.review_session(&id)?),
                        details,
                        style,
                    )?;
                }
            }
            InboxCommand::Apply { id } => {
                ensure!(
                    io::stdin().is_terminal(),
                    "approval requires an interactive terminal; inspect the plan and run this command locally"
                );
                let plan = if vault.has_review(&id)? {
                    vault.prepare_review_plan(&id)?
                } else {
                    vault.load_plan(&id)?
                };
                terminal::plan(&mut io::stderr().lock(), &plan, None, true, style)?;
                approve(&plan.id)?;
                emit!(vault.apply_plan(&plan.id)?);
            }
            InboxCommand::Undo { id } => {
                ensure!(
                    io::stdin().is_terminal(),
                    "Undo requires an interactive terminal; open inbox review and choose Undo last"
                );
                let undo = vault.review_undo_plan(&id)?;
                terminal::changes(&mut io::stderr().lock(), &undo.changes, style)?;
                approve(&undo.id)?;
                emit!(vault.undo_review_item(&id, &undo.id)?);
            }
            InboxCommand::MarkReviewed => emit!(vault.mark_reviewed()?),
        },
        Command::Reminder { command } => match command {
            ReminderCommand::Configure { every_days } => {
                emit!(vault.configure_reminder(Some(every_days))?);
            }
            ReminderCommand::Disable => emit!(vault.configure_reminder(None)?),
            ReminderCommand::Check { exit_code } => {
                let status = vault.inbox_status()?;
                if cli.json {
                    emit!(status);
                }
                if status["due"] == true {
                    if !cli.json {
                        println!(
                            "Inbox review is due: {} captures. Run mcp-brain --vault <path> inbox review.",
                            status["count"]
                        );
                    }
                    if exit_code {
                        std::process::exit(3);
                    }
                }
            }
            ReminderCommand::Calendar { time } => {
                ensure!(!cli.json, "calendar exports iCalendar; omit --json");
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
