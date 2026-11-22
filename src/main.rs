use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    style::{Attribute, ResetColor, SetAttribute},
    terminal::{
        Clear, ClearType, EnableLineWrap, EndSynchronizedUpdate, EnterAlternateScreen,
        LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
};
use std::{
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tachyonfx::fx;
use tootles::{
    app::{App, parse_theme, theme_key},
    auth,
    config::Config,
    player::{self, TerminalOutput},
    ui,
};

#[derive(Parser)]
#[command(
    name = "tootles",
    version,
    about = "Your corner of the open web. A keyboard-first Mastodon client.",
    long_about = "A calm, capable terminal home for Mastodon. Run with no arguments to open your account; without an account, Tootles opens its interactive offline demo."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Explore a fully interactive offline playground
    #[arg(long)]
    demo: bool,
    /// Use a separate configuration directory
    #[arg(long, global = true, env = "TOOTLES_CONFIG_DIR")]
    config_dir: Option<PathBuf>,
    /// tokyo-night (default), midnight, paper, or contrast
    #[arg(long,value_parser=["tokyo-night","midnight","paper","contrast"])]
    theme: Option<String>,
    /// Disable visual transitions
    #[arg(long)]
    no_motion: bool,
    /// Leave mouse input to the terminal (useful for text selection)
    #[arg(long)]
    no_mouse: bool,
    /// Use halfblocks to skip image capability queries (helpful over SSH/tmux)
    #[arg(long, value_parser=["auto", "halfblocks"], default_value="auto")]
    image_protocol: String,
    /// Render an offline demo snapshot to .svg or .txt without a terminal
    #[arg(long)]
    snapshot: Option<PathBuf>,
    #[arg(long, default_value_t = 144)]
    width: u16,
    #[arg(long, default_value_t = 44)]
    height: u16,
}
#[derive(Subcommand)]
enum Commands {
    /// Connect a Mastodon account using browser OAuth
    Login {
        /// Your server, for example mastodon.social
        instance: String,
        /// Read an existing access token from standard input (never a command argument)
        #[arg(long)]
        token_stdin: bool,
        /// Friendly name for the account switcher
        #[arg(long)]
        label: Option<String>,
    },
    /// List connected accounts without revealing credentials
    Accounts,
    /// Choose the account used on the next launch
    Switch {
        /// Account number, label, or ID from `tootles accounts`
        account: String,
    },
    /// Remove an account's locally saved access token
    Logout {
        /// Account number, label, or ID; defaults to the active account
        account: Option<String>,
    },
    /// Save your preferred color theme
    Theme {
        #[arg(value_parser=["tokyo-night","midnight","paper","contrast"])]
        name: String,
    },
    /// Print installation and terminal diagnostics
    Doctor,
    /// Generate shell completion definitions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("tootles: {err:#}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Commands::Completions { shell }) = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "tootles", &mut io::stdout());
        return Ok(());
    }
    // Snapshot rendering is deterministic and never reads saved credentials.
    let mut config = if cli.snapshot.is_some() {
        Config::default()
    } else {
        Config::load(cli.config_dir.as_deref())?
    };
    if let Some(command) = cli.command {
        match command {
            Commands::Login {
                instance,
                token_stdin,
                label,
            } => {
                let token = if token_stdin {
                    let mut token = String::new();
                    io::stdin().take(16384).read_to_string(&mut token)?;
                    Some(token)
                } else {
                    std::env::var("TOOTLES_TOKEN").ok()
                };
                let account = auth::login(&instance, token, label).await?;
                config.accounts.retain(|a| a.id != account.id);
                config.active = Some(account.id.clone());
                let label = account.label.clone();
                config.accounts.push(account);
                config.save(cli.config_dir.as_deref())?;
                println!("Connected {label}. Run `tootles` to make yourself at home.");
            }
            Commands::Accounts => {
                if config.accounts.is_empty() {
                    println!("No accounts yet. Connect with: tootles login mastodon.social");
                }
                for (i, a) in config.accounts.iter().enumerate() {
                    let active = config.active_account().is_some_and(|x| x.id == a.id);
                    println!(
                        "{} {}  {}  ({})",
                        if active { "*" } else { " " },
                        i + 1,
                        a.label,
                        a.instance
                    );
                }
            }
            Commands::Switch { account } => {
                let i = find_account(&config, &account)?;
                config.active = Some(config.accounts[i].id.clone());
                config.save(cli.config_dir.as_deref())?;
                println!("Active account: {}", config.accounts[i].label);
            }
            Commands::Logout { account } => {
                let id = if let Some(selector) = account {
                    config.accounts[find_account(&config, &selector)?]
                        .id
                        .clone()
                } else {
                    config
                        .active_account()
                        .context("No connected account")?
                        .id
                        .clone()
                };
                config.accounts.retain(|a| a.id != id);
                if config.active.as_ref() == Some(&id) {
                    config.active = config.accounts.first().map(|a| a.id.clone());
                }
                config.save(cli.config_dir.as_deref())?;
                println!(
                    "Removed the locally saved token. You can revoke Tootles in your server's authorized apps settings. Drafts remain on this device."
                );
            }
            Commands::Theme { name } => {
                config.theme = name.clone();
                config.save(cli.config_dir.as_deref())?;
                println!("Theme: {}", parse_theme(&name).name());
            }
            Commands::Doctor => {
                println!(
                    "Tootles {}\nConfig: {}\nAccounts: {}\nTheme: {}\nTerminal: {}\nTrue color: {}\nWayland: {}\nInteractive terminal: {}",
                    env!("CARGO_PKG_VERSION"),
                    tootles::config::config_dir(cli.config_dir.as_deref())?.display(),
                    config.accounts.len(),
                    config.theme,
                    std::env::var("TERM").unwrap_or_else(|_| "unset".into()),
                    std::env::var("COLORTERM").unwrap_or_else(|_| "unset".into()),
                    std::env::var_os("WAYLAND_DISPLAY").is_some(),
                    io::stdin().is_terminal() && io::stdout().is_terminal()
                );
                println!(
                    "\nTip: Kitty, Ghostty, WezTerm, and foot work well under Hyprland.\nRun `tootles --demo` to check the interface. Press ? for shortcuts."
                );
            }
            Commands::Completions { .. } => unreachable!(),
        }
        return Ok(());
    }
    if let Some(theme) = cli.theme {
        config.theme = theme;
    }
    if cli.no_motion || std::env::var_os("NO_COLOR").is_some() {
        config.reduced_motion = true;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        config.theme = "contrast".into();
    }
    let config_root = if cli.snapshot.is_some() {
        Some(std::env::temp_dir().join(format!("tootles-snapshot-{}", uuid::Uuid::new_v4())))
    } else {
        cli.config_dir
    };
    let mut app = App::new(config, config_root, cli.demo || cli.snapshot.is_some())?;
    if let Some(path) = cli.snapshot {
        if cli.width < 20 || cli.height < 8 || cli.width > 400 || cli.height > 200 {
            bail!("Snapshot dimensions must be 20–400 columns and 8–200 rows");
        }
        app.prepare_snapshot().await?;
        let mut terminal = Terminal::new(TestBackend::new(cli.width, cli.height))?;
        terminal.draw(|frame| ui::render(frame, &mut app.view))?;
        tootles::snapshot::save(terminal.backend().buffer(), &path)?;
        println!("Wrote {} ({})", path.display(), theme_key(app.view.theme));
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!(
            "The interface needs an interactive terminal. Try `tootles --help` or `tootles --snapshot preview.svg`."
        );
    }
    tui(&mut app, !cli.no_mouse, &cli.image_protocol).await
}
fn find_account(config: &Config, selector: &str) -> Result<usize> {
    if let Ok(n) = selector.parse::<usize>()
        && n > 0
        && n <= config.accounts.len()
    {
        return Ok(n - 1);
    }
    let matches: Vec<_> = config
        .accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.id == selector || a.label == selector)
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => bail!("Account not found; use `tootles accounts`"),
        _ => bail!("That label matches multiple accounts; use its account number"),
    }
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            crossterm::cursor::Show,
            LeaveAlternateScreen
        );
    }
}

/// Resume even when a player launch or an early return fails. The outer guard
/// still restores the original shell if resuming the TUI itself fails.
struct TerminalHandoff {
    mouse: bool,
    resumed: bool,
}

impl TerminalHandoff {
    fn suspend(mouse: bool) -> Result<Self> {
        let handoff = Self {
            mouse,
            resumed: false,
        };
        execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            EndSynchronizedUpdate,
            SetAttribute(Attribute::Reset),
            ResetColor,
            Clear(ClearType::All),
            crossterm::cursor::MoveTo(0, 0),
            crossterm::cursor::Show
        )?;
        // Keep playback in the application screen. Leaving it here exposes
        // the shell prompt and writes the playback banner into shell history.
        // Keep raw mode too: mpv inherits it and restores it on exit, without
        // an echo-enabled gap. Crossterm retains the original shell settings.
        Ok(handoff)
    }

    fn resume(&mut self) -> Result<()> {
        // A child can change tty settings even if crossterm still considers
        // raw mode enabled. Restore our saved baseline before applying it again.
        disable_raw_mode()?;
        enable_raw_mode()?;
        // Cancel an unfinished control sequence and restore normal keypad
        // mode even if the player exited without its normal cleanup.
        io::stdout().write_all(b"\x18\x1b>")?;
        execute!(
            io::stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            SetAttribute(Attribute::Reset),
            ResetColor,
            EnableLineWrap,
            // mpv's text driver leaves the alternate screen on exit.
            EnterAlternateScreen,
            EnableBracketedPaste,
            crossterm::cursor::Hide
        )?;
        if self.mouse {
            execute!(io::stdout(), EnableMouseCapture)?;
        }
        self.resumed = true;
        Ok(())
    }
}

impl Drop for TerminalHandoff {
    fn drop(&mut self) {
        if !self.resumed {
            let _ = self.resume();
        }
    }
}

async fn tui(app: &mut App, mouse: bool, image_protocol: &str) -> Result<()> {
    app.view.mouse_enabled = mouse;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            crossterm::cursor::Show,
            LeaveAlternateScreen
        );
        previous(info);
    }));
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    if mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let picker = if image_protocol == "halfblocks" {
        ratatui_image::picker::Picker::halfblocks()
    } else {
        ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks())
    };
    let terminal_output = if picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty {
        TerminalOutput::Kitty
    } else {
        TerminalOutput::TrueColor
    };
    let mut gallery: Option<tootles::media::Gallery> = None;
    // Poll nonblocking on this task: no detached EventStream reader can retain
    // stdin while mpv temporarily owns the terminal. Idle polls do not redraw.
    let mut input = tokio::time::interval(Duration::from_millis(16));
    input.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut interval = tokio::time::interval(Duration::from_millis(80));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut effect = fx::fade_from_fg(ratatui::style::Color::Rgb(26, 27, 38), 300);
    let mut last_frame = std::time::Instant::now();
    let mut redraw = true;
    app.initialize();
    while !app.quit {
        if redraw {
            let elapsed = last_frame.elapsed();
            last_frame = std::time::Instant::now();
            terminal.draw(|frame| {
                ui::render(frame, &mut app.view);
                if !app.view.reduced_motion && !effect.done() {
                    let area = frame.area();
                    effect.process(elapsed.into(), frame.buffer_mut(), area);
                }
                if let Some(gallery) = &mut gallery {
                    gallery.render(frame);
                }
            })?;
            redraw = false;
        }
        tokio::select! {
            _=interval.tick()=>{
                app.tick();
                if let Some(g)=&mut gallery { g.tick(); }
                redraw=true;
            },
            _=input.tick()=>{
                if crossterm::event::poll(Duration::ZERO)? {
                    let event=crossterm::event::read()?;
                    if let Some(g)=&mut gallery {
                        if g.event(&event) { gallery=None; terminal.clear()?; }
                    } else {
                        let width=terminal.size()?.width;
                        app.event(event,width);
                    }
                    redraw=true;
                }
            },
            _=tokio::signal::ctrl_c()=>app.command("quit"),
        }
        if let Some(post) = app.media_requested.take() {
            gallery = Some(
                tootles::media::Gallery::new(post, picker.clone())
                    .with_config_root(app.config_root().map(Path::to_path_buf)),
            );
            redraw = true;
        }
        if let Some(path) = app.local_media_requested.take() {
            gallery = Some(
                tootles::media::Gallery::from_file(path, picker.clone())
                    .with_config_root(app.config_root().map(Path::to_path_buf)),
            );
            redraw = true;
        }
        if let Some(source) = gallery.as_mut().and_then(|g| g.take_terminal_request()) {
            let mut handoff = TerminalHandoff::suspend(mouse)?;
            write!(
                io::stdout(),
                "Tootles · Space pause · arrows seek · 9/0 volume · q return\r\n"
            )?;
            io::stdout().flush()?;
            let result = player::play_terminal(&source, terminal_output).await;
            handoff.resume()?;
            terminal.clear()?;
            if let Some(gallery) = &mut gallery {
                gallery.playback_finished(result);
            }
            redraw = true;
        }
    }
    Ok(())
}
